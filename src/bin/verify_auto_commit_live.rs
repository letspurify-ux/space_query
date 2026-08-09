#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the auto-commit model across Oracle Thin, Oracle OCI,
// MySQL and MariaDB. Drives the real SqlEditorWidget like the GUI does.
//
// Scenarios per target:
//   S1  connection auto-commit ON: DML really commits (survives ROLLBACK),
//       result carries "Auto-commit applied", tab close needs no prompt.
//   S2  tab scope: with the connection default OFF, script SET AUTOCOMMIT ON
//       commits this tab's DML but leaves the connection default untouched.
//   S3  dirty guard: with manual commit and an open transaction, script
//       SET AUTOCOMMIT ON must be refused — and must stay without effect
//       (a following ROLLBACK still undoes the work).
//   S4  (Oracle only) SET TRANSACTION READ ONLY under auto-commit ON: the
//       next INSERT must fail with ORA-01456. Before the thin skip_auto_commit
//       fix the execute message piggybacked a commit that silently ended the
//       read-only transaction, so the INSERT wrongly succeeded.
//   S5  (MySQL family) the dirty-probe SQL of each dialect answers correctly
//       on a live server: MariaDB @@in_transaction; MySQL innodb_trx (and
//       @@in_transaction must error there).
//
// Usage: verify_auto_commit_live <thin|oci|mysql|mariadb|all>

use fltk::{app, input::IntInput};
use mysql::prelude::Queryable;
use space_query::db::{
    retained_session_state_preflight_decision, ConnectionInfo, DatabaseConnection, DatabaseType,
    OracleDriverMode, QueryResult, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision,
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
                "DROP TABLE SQ_AC_T".into(),
                "CREATE TABLE SQ_AC_T (V NUMBER)".into(),
                "INSERT INTO SQ_AC_T VALUES (1)".into(),
                "COMMIT".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_AC_T".into(),
                "CREATE TABLE SQ_AC_T (V INT)".into(),
                "INSERT INTO SQ_AC_T VALUES (1)".into(),
                "COMMIT".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec!["DROP TABLE SQ_AC_T".into()]
        } else {
            vec!["DROP TABLE IF EXISTS SQ_AC_T".into()]
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

#[derive(Default)]
struct RunCapture {
    results: Vec<QueryResult>,
    messages: Vec<String>,
    rows: Vec<Vec<String>>,
}

struct Harness {
    editor: SqlEditorWidget,
    done: Arc<AtomicBool>,
    capture: Arc<Mutex<RunCapture>>,
    shared: space_query::db::SharedConnection,
    failures: Vec<String>,
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

    fn run(&mut self, sql: &str) -> Result<RunCapture, String> {
        self.done.store(false, Ordering::SeqCst);
        *self.capture.lock().unwrap_or_else(|p| p.into_inner()) = RunCapture::default();
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("statement to finish", || done.load(Ordering::SeqCst))?;
        if env::var("SQ_TRACE_EVENTS").is_ok() {
            println!(
                "    (state after {:?}) {:?}",
                sql.lines().next().unwrap_or(""),
                self.editor
                    .pooled_session_activity_snapshot()
                    .map(|s| s.retained_state().transaction_state())
            );
        }
        Ok(std::mem::take(
            &mut *self.capture.lock().unwrap_or_else(|p| p.into_inner()),
        ))
    }

    fn close_would_prompt(&self) -> bool {
        self.editor
            .pooled_session_activity_snapshot()
            .map(|snap| {
                retained_session_state_preflight_decision(
                    RetainedSessionPreflightAction::Close,
                    snap.retained_state(),
                ) == RetainedSessionPreflightDecision::RequireResolution
            })
            .unwrap_or(false)
    }

    fn connection_auto_commit(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auto_commit()
    }

    fn set_connection_auto_commit(&mut self, enabled: bool) -> Result<(), String> {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .set_auto_commit(enabled)
    }

    fn select_v(&mut self) -> Result<i64, String> {
        let capture = self.run("SELECT V FROM SQ_AC_T")?;
        // The row may carry a leading hidden ROWID column; V is the last cell.
        let from_streamed = capture.rows.first().and_then(|row| row.last()).cloned();
        let from_result = capture
            .results
            .iter()
            .find(|r| r.is_select)
            .and_then(|r| r.rows.first())
            .and_then(|row| row.last())
            .cloned();
        let cell = from_streamed
            .or(from_result)
            .ok_or("SELECT V returned no rows")?;
        cell.trim()
            .parse::<i64>()
            .map_err(|e| format!("SELECT V returned {cell:?}: {e}"))
    }

    fn check(&mut self, label: &str, ok: bool, detail: String) {
        if ok {
            println!("    OK  {label}");
        } else {
            println!("    FAIL {label}: {detail}");
            self.failures.push(format!("{label}: {detail}"));
        }
    }
}

fn run_scenarios(target: Target, h: &mut Harness) -> Result<(), String> {
    let dml = "UPDATE SQ_AC_T SET V = V + 1";

    // ---- S1: connection default ON commits for real -----------------------
    println!("  --- S1 connection auto-commit ON ---");
    h.set_connection_auto_commit(true)
        .map_err(|e| format!("set_auto_commit(true): {e}"))?;
    let capture = h.run(dml)?;
    let dml_result = capture
        .results
        .first()
        .ok_or("S1 DML produced no result")?
        .clone();
    let auto_commit_applied = space_query::db::result_messages::AUTO_COMMIT_APPLIED;
    h.check(
        "S1 DML succeeds with the auto-commit-applied feedback",
        dml_result.success && dml_result.message.contains(auto_commit_applied),
        format!(
            "success={} message={:?}",
            dml_result.success, dml_result.message
        ),
    );
    let no_prompt = !h.close_would_prompt();
    h.check(
        "S1 close needs no prompt",
        no_prompt,
        "close preflight still requires resolution".into(),
    );
    h.run("ROLLBACK")?;
    let v = h.select_v()?;
    h.check(
        "S1 value survived ROLLBACK (really committed)",
        v == 2,
        format!("expected 2, got {v}"),
    );

    // ---- S2: script SET AUTOCOMMIT is tab-scoped --------------------------
    println!("  --- S2 tab-scoped SET AUTOCOMMIT ---");
    h.set_connection_auto_commit(false)
        .map_err(|e| format!("set_auto_commit(false): {e}"))?;
    h.run("SET AUTOCOMMIT ON")?;
    let default_untouched = !h.connection_auto_commit();
    h.check(
        "S2 connection default untouched by script",
        default_untouched,
        "script SET AUTOCOMMIT mutated the shared connection default".into(),
    );
    h.run(dml)?;
    h.run("ROLLBACK")?;
    let v = h.select_v()?;
    h.check(
        "S2 tab override committed the DML",
        v == 3,
        format!("expected 3, got {v}"),
    );

    // ---- S3: dirty guard refuses SET AUTOCOMMIT and stays without effect --
    println!("  --- S3 dirty-transaction guard ---");
    h.run("SET AUTOCOMMIT OFF")?;
    h.run(dml)?; // open transaction, manual mode
    let capture = h.run("SET AUTOCOMMIT ON")?;
    let refused = capture
        .results
        .iter()
        .any(|r| !r.success && r.message.contains("auto-commit"))
        || capture
            .messages
            .iter()
            .any(|m| m.contains("Cannot change auto-commit") || m.contains("Error"));
    h.check(
        "S3 SET AUTOCOMMIT refused while dirty",
        refused,
        format!(
            "results={:?} messages={:?}",
            capture
                .results
                .iter()
                .map(|r| (r.success, r.message.clone()))
                .collect::<Vec<_>>(),
            capture.messages
        ),
    );
    h.run("ROLLBACK")?;
    let v = h.select_v()?;
    h.check(
        "S3 refused change had no effect (ROLLBACK undid the DML)",
        v == 3,
        format!("expected 3, got {v}"),
    );

    // ---- S6 (Oracle, runs last): script CONNECT resolves auto-commit from
    // the new connection's seeded default, not the old connection's value ----
    fn run_connect_scenario(target: Target, h: &mut Harness) -> Result<(), String> {
        println!("  --- S6 script CONNECT re-resolves auto-commit ---");
        // Old connection ON, no tab override: after CONNECT the batch must
        // switch to the new connection's seeded default (config default: OFF).
        // Before the fix the batch kept the stale ON and wrongly committed.
        h.editor.sync_tab_auto_commit_with_global_setting(false);
        h.set_connection_auto_commit(true)
            .map_err(|e| format!("set_auto_commit(true): {e}"))?;
        let info = target.connection_info();
        let script = format!(
            "CONNECT {}/{}@{}:{}/{}\nUPDATE SQ_AC_T SET V = V + 1;",
            info.username, info.password, info.host, info.port, info.service_name
        );
        let capture = h.run(&script)?;
        let dml_result = capture
            .results
            .iter()
            .find(|r| r.sql.contains("UPDATE"))
            .cloned()
            .ok_or("S6 DML after CONNECT produced no result")?;
        let commit_required = space_query::db::result_messages::COMMIT_REQUIRED;
        h.check(
            "S6 DML after CONNECT runs in the new connection's manual mode",
            dml_result.success && dml_result.message.contains(commit_required),
            format!(
                "success={} message={:?}",
                dml_result.success, dml_result.message
            ),
        );
        h.run("ROLLBACK")?;
        let v = h.select_v()?;
        h.check(
            "S6 ROLLBACK undid the DML (stale auto-commit did not leak across CONNECT)",
            v == 3,
            format!("expected 3, got {v}"),
        );
        Ok(())
    }

    // ---- S4 (Oracle): READ ONLY transaction vs piggybacked commit ---------
    if target.is_oracle() {
        println!("  --- S4 SET TRANSACTION READ ONLY under auto-commit ON ---");
        h.run("SET AUTOCOMMIT ON")?;
        let capture = h.run("SET TRANSACTION READ ONLY;\nINSERT INTO SQ_AC_T VALUES (99);")?;
        let set_txn_ok = capture.results.first().map(|r| r.success).unwrap_or(false);
        let insert_result = capture.results.get(1);
        let insert_failed_read_only = insert_result
            .map(|r| !r.success && r.message.contains("ORA-01456"))
            .unwrap_or(false);
        h.check(
            "S4 SET TRANSACTION READ ONLY succeeded",
            set_txn_ok,
            format!("results={:?}", capture.results.first().map(|r| &r.message)),
        );
        h.check(
            "S4 INSERT failed with ORA-01456 (no piggybacked commit ended the read-only txn)",
            insert_failed_read_only,
            format!(
                "insert result: {:?}",
                insert_result.map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("COMMIT")?;
        let v = h.select_v()?;
        h.check("S4 table unchanged", v == 3, format!("expected 3, got {v}"));

        // Last on purpose: CONNECT rebinds the tab to a transient connection,
        // so the harness's original shared connection no longer matches the
        // executing one afterwards.
        run_connect_scenario(target, h)?;
    }

    Ok(())
}

fn verify_mysql_probe_sql(target: Target) -> Result<Vec<String>, String> {
    let info = target.connection_info();
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(info.host.clone()))
        .tcp_port(info.port)
        .user(Some(info.username.clone()))
        .pass(Some(info.password.clone()))
        .db_name(Some(info.service_name.clone()));
    let mut conn = mysql::Conn::new(opts).map_err(|e| format!("raw connect: {e}"))?;
    let mut failures = Vec::new();

    conn.query_drop("DROP TABLE IF EXISTS SQ_AC_PROBE_T")
        .and_then(|_| conn.query_drop("CREATE TABLE SQ_AC_PROBE_T (V INT)"))
        .map_err(|e| format!("probe setup: {e}"))?;

    // Probe with exactly the SQL the app ships, so this check cannot drift
    // from the production probe.
    let db_type = if target == Target::MariaDb {
        DatabaseType::MariaDB
    } else {
        DatabaseType::MySQL
    };
    let primary_probe_sql = DatabaseConnection::mysql_transaction_probe_sql_order(db_type)[0];
    let primary_probe = |conn: &mut mysql::Conn| conn.query_first::<u64, _>(primary_probe_sql);

    conn.query_drop("SET autocommit=0")
        .map_err(|e| format!("disable autocommit: {e}"))?;

    // Trivial statements (the shape of app metadata work) must not read as
    // dirty, or the auto-commit toggle would be refused for a clean session.
    // (A real-table read under autocommit=0 genuinely opens a transaction on
    // both servers; the app avoids that by keeping the live metadata
    // connection on autocommit=1.)
    conn.query_first::<u64, _>("SELECT 1")
        .map_err(|e| format!("read query: {e}"))?;
    match primary_probe(&mut conn) {
        Ok(Some(0)) | Ok(None) => println!("    OK  trivial statement reads as clean"),
        other => failures.push(format!("probe after trivial statement: {other:?}")),
    }

    conn.query_drop("START TRANSACTION")
        .and_then(|_| conn.query_drop("INSERT INTO SQ_AC_PROBE_T VALUES (1)"))
        .map_err(|e| format!("open transaction: {e}"))?;
    match primary_probe(&mut conn) {
        Ok(Some(n)) if n >= 1 => println!("    OK  uncommitted INSERT reads as dirty"),
        other => failures.push(format!("probe with uncommitted INSERT: {other:?}")),
    }
    if target == Target::MySql {
        match conn.query_first::<u64, _>("SELECT @@in_transaction") {
            Err(_) => println!("    OK  MySQL @@in_transaction errors (fallback order matters)"),
            other => failures.push(format!(
                "MySQL @@in_transaction unexpectedly answered: {other:?}"
            )),
        }
    }

    conn.query_drop("ROLLBACK")
        .map_err(|e| format!("rollback: {e}"))?;
    match primary_probe(&mut conn) {
        Ok(Some(0)) | Ok(None) => println!("    OK  probe reports clean after ROLLBACK"),
        other => failures.push(format!("probe after rollback: {other:?}")),
    }
    let _ = conn.query_drop("DROP TABLE IF EXISTS SQ_AC_PROBE_T");
    Ok(failures)
}

fn verify(target: Target) -> Result<Vec<String>, String> {
    println!("\n########## {} ##########", target.label());

    let mut connection = DatabaseConnection::new();
    connection
        .connect(target.connection_info())
        .map_err(|e| format!("connect: {e}"))?;
    let shared: space_query::db::SharedConnection = Arc::new(Mutex::new(connection));

    let timeout_input = IntInput::default();
    let mut editor = SqlEditorWidget::new(Arc::clone(&shared), timeout_input);
    let done = Arc::new(AtomicBool::new(false));
    let capture: Arc<Mutex<RunCapture>> = Arc::new(Mutex::new(RunCapture::default()));
    {
        let done = Arc::clone(&done);
        let capture = Arc::clone(&capture);
        editor.set_progress_callback(move |event| {
            if env::var("SQ_TRACE_EVENTS").is_ok() {
                let name = match progress_inner(&event) {
                    QueryProgress::Message { .. } => "Message",
                    QueryProgress::StatementFinished { result, .. } => {
                        println!(
                            "    (statement) success={} msg={:?}",
                            result.success, result.message
                        );
                        "StatementFinished"
                    }
                    QueryProgress::BatchFinished => "BatchFinished",
                    QueryProgress::ExecutionFinished(_) => "ExecutionFinished",
                    _ => "Other",
                };
                println!("    (event) {name}");
            }
            match progress_inner(&event) {
                QueryProgress::Message { lines, .. }
                | QueryProgress::ScriptOutput { lines, .. } => {
                    capture
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .messages
                        .extend(lines.iter().cloned());
                }
                QueryProgress::StatementFinished { result, .. } => {
                    capture
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .results
                        .push(result.clone());
                }
                QueryProgress::Rows { rows, .. } => {
                    capture
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .rows
                        .extend(rows.iter().cloned());
                }
                QueryProgress::BatchFinished => {
                    done.store(true, Ordering::SeqCst);
                }
                _ => {}
            }
        });
    }
    let mut h = Harness {
        editor,
        done,
        capture,
        shared,
        failures: Vec::new(),
    };

    for (i, sql) in target.setup().into_iter().enumerate() {
        let r = h.run(&sql);
        if i >= 1 {
            r.map_err(|e| format!("setup {sql:?}: {e}"))?;
        }
    }

    let scenario_result = run_scenarios(target, &mut h);

    for sql in target.teardown() {
        let _ = h.run(&sql);
    }
    scenario_result?;

    let mut failures = h.failures;
    if !target.is_oracle() {
        println!("  --- S5 dirty-probe SQL on the live server ---");
        failures.extend(verify_mysql_probe_sql(target)?);
    }
    Ok(failures)
}

fn main() {
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
            eprintln!("unknown target {other}; use thin|oci|mysql|mariadb|all");
            std::process::exit(2);
        }
    };

    let _app = app::App::default();
    let mut all_failures = Vec::new();
    for target in targets {
        match verify(target) {
            Ok(failures) if failures.is_empty() => {
                println!("== {} PASSED ==", target.label());
            }
            Ok(failures) => {
                println!("== {} FAILED ==", target.label());
                for f in &failures {
                    println!("   - {f}");
                }
                all_failures.extend(
                    failures
                        .into_iter()
                        .map(|f| format!("{}: {f}", target.label())),
                );
            }
            Err(err) => {
                println!("== {} ERROR: {err} ==", target.label());
                all_failures.push(format!("{}: {err}", target.label()));
            }
        }
    }
    if all_failures.is_empty() {
        println!("\nALL AUTO-COMMIT LIVE CHECKS PASSED");
    } else {
        println!("\nFAILURES:");
        for f in &all_failures {
            println!(" - {f}");
        }
        std::process::exit(1);
    }
}
