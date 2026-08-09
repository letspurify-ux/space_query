#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the tab-scoped transaction-mode model across Oracle
// Thin, Oracle OCI, MySQL and MariaDB. Drives the real SqlEditorWidget like
// the GUI does.
//
// Scenarios per target:
//   S1  tab scope: pinning the tab to READ ONLY (the toolbar write path) makes
//       the next INSERT fail on the server (ORA-01456 / MySQL 1792), while the
//       shared connection's transaction mode stays untouched; unpinning (back
//       to the connection default) lets the INSERT succeed again.
//   S2  query-driven UI sync: a successful session-scoped statement
//       (SET SESSION TRANSACTION ISOLATION LEVEL ... / ALTER SESSION SET
//       ISOLATION_LEVEL ...) pins the tab override AND emits
//       TransactionModeChanged so the toolbar re-syncs immediately.
//   S3  (MySQL family) the adopted isolation is really applied to the tab's
//       session on the next execution (SELECT @@transaction_isolation), i.e.
//       screen = tab state = applied session state.
//   S4  one-shot SET TRANSACTION (next-transaction scope) must NOT repin the
//       tab or emit TransactionModeChanged.
//
// Usage: verify_transaction_mode_live <thin|oci|mysql|mariadb|all>

use fltk::{app, input::IntInput};
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode, QueryResult,
    TransactionAccessMode, TransactionIsolation, TransactionMode,
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
                "DROP TABLE SQ_TM_T".into(),
                "CREATE TABLE SQ_TM_T (V NUMBER)".into(),
                "INSERT INTO SQ_TM_T VALUES (1)".into(),
                "COMMIT".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TM_T".into(),
                "CREATE TABLE SQ_TM_T (V INT)".into(),
                "INSERT INTO SQ_TM_T VALUES (1)".into(),
                "COMMIT".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec!["DROP TABLE SQ_TM_T".into()]
        } else {
            vec!["DROP TABLE IF EXISTS SQ_TM_T".into()]
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
    mode_changes: Vec<TransactionMode>,
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
        Ok(std::mem::take(
            &mut *self.capture.lock().unwrap_or_else(|p| p.into_inner()),
        ))
    }

    fn connection_transaction_mode(&self) -> TransactionMode {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .transaction_mode()
    }

    fn select_v(&mut self) -> Result<i64, String> {
        let capture = self.run("SELECT V FROM SQ_TM_T")?;
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

    fn select_scalar(&mut self, sql: &str) -> Result<String, String> {
        let capture = self.run(sql)?;
        let from_streamed = capture.rows.first().and_then(|row| row.last()).cloned();
        let from_result = capture
            .results
            .iter()
            .find(|r| r.is_select)
            .and_then(|r| r.rows.first())
            .and_then(|row| row.last())
            .cloned();
        from_streamed
            .or(from_result)
            .ok_or_else(|| format!("{sql} returned no rows"))
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
    let initial_connection_mode = h.connection_transaction_mode();
    // Oracle: either the server rejects the write inside the READ ONLY
    // transaction (ORA-01456) or the app's own read-only gate refuses the
    // non-query client-side before it reaches the server. MySQL family:
    // ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION.
    let read_only_errors: &[&str] = if target.is_oracle() {
        &["ORA-01456", "read-only mode blocks"]
    } else {
        &["read only"]
    };

    // ---- S1: tab-scoped READ ONLY via the toolbar write path --------------
    println!("  --- S1 tab-scoped READ ONLY pin ---");
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    if !target.is_oracle() {
        let read_only_var = if target == Target::MariaDb {
            "SELECT @@tx_read_only"
        } else {
            "SELECT @@transaction_read_only"
        };
        let value = h.select_scalar(read_only_var)?;
        h.check(
            "S1 session variable reports read-only after the pin",
            matches!(value.trim(), "1" | "ON"),
            format!("{read_only_var} = {value:?}"),
        );
    }
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (99)")?;
    let insert_result = capture.results.first().cloned();
    let insert_refused = insert_result
        .as_ref()
        .map(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        })
        .unwrap_or(false);
    h.check(
        "S1 INSERT fails on the read-only tab session",
        insert_refused,
        format!(
            "insert result: {:?}",
            insert_result.map(|r| (r.success, r.message))
        ),
    );
    h.check(
        "S1 shared connection transaction mode untouched",
        h.connection_transaction_mode() == initial_connection_mode,
        format!(
            "connection mode changed to {:?}",
            h.connection_transaction_mode()
        ),
    );
    // The refused INSERT leaves the read-only transaction open on both
    // families; end it so the unpinned write starts a fresh transaction.
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (2)")?;
    let insert_ok = capture.results.first().map(|r| r.success).unwrap_or(false);
    h.check(
        "S1 unpinned tab (connection default) can write again",
        insert_ok,
        format!(
            "insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("COMMIT")?;
    let v = h.select_v()?;
    h.check("S1 committed row visible", v == 1, format!("first V = {v}"));

    // ---- S2: query-driven session-scoped change pins the tab + notifies UI -
    println!("  --- S2 session-scoped statement adopts into the tab + UI ---");
    let (session_isolation_sql, expected_isolation) = if target.is_oracle() {
        (
            "ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE",
            TransactionIsolation::Serializable,
        )
    } else {
        (
            "SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            TransactionIsolation::Serializable,
        )
    };
    let capture = h.run(session_isolation_sql)?;
    let statement_ok = capture.results.first().map(|r| r.success).unwrap_or(true);
    h.check(
        "S2 session-scoped statement succeeded",
        statement_ok,
        format!(
            "result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    let override_mode = h.editor.tab_transaction_mode_override_value();
    h.check(
        "S2 tab override pinned to the adopted isolation",
        override_mode.map(|m| m.isolation) == Some(expected_isolation),
        format!("override: {override_mode:?}"),
    );
    h.check(
        "S2 TransactionModeChanged event notified the UI",
        capture
            .mode_changes
            .iter()
            .any(|mode| mode.isolation == expected_isolation),
        format!("mode change events: {:?}", capture.mode_changes),
    );
    h.check(
        "S2 shared connection transaction mode still untouched",
        h.connection_transaction_mode() == initial_connection_mode,
        format!(
            "connection mode changed to {:?}",
            h.connection_transaction_mode()
        ),
    );

    // ---- S3 (MySQL family): the adopted mode is applied on the next run ----
    if !target.is_oracle() {
        println!("  --- S3 adopted isolation persists on the tab session ---");
        let isolation_var = if target == Target::MariaDb {
            "SELECT @@tx_isolation"
        } else {
            "SELECT @@transaction_isolation"
        };
        let value = h.select_scalar(isolation_var)?;
        h.check(
            "S3 next execution observes SERIALIZABLE",
            value
                .replace(['-', '_'], " ")
                .eq_ignore_ascii_case("SERIALIZABLE"),
            format!("{isolation_var} = {value:?}"),
        );
        // The SELECT under autocommit-off opened a transaction; end it so S4's
        // SET TRANSACTION runs outside a transaction.
        h.run("ROLLBACK")?;
    } else {
        // Oracle: prove the pinned isolation is re-applied by the next batch —
        // a serializable transaction cannot see a row committed by another
        // session after the transaction began. Simpler smoke: the pinned
        // override survives and the next statement still succeeds.
        println!("  --- S3 pinned isolation still lets statements run ---");
        let v = h.select_v()?;
        h.check(
            "S3 SELECT under SERIALIZABLE works",
            v == 1,
            format!("V = {v}"),
        );
        if target.is_oracle() {
            h.run("COMMIT")?;
        }
    }
    h.editor.clear_tab_transaction_mode_override();

    // ---- S4: one-shot SET TRANSACTION must not repin the tab --------------
    println!("  --- S4 one-shot SET TRANSACTION does not repin ---");
    let one_shot = if target.is_oracle() {
        "SET TRANSACTION READ ONLY"
    } else {
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
    };
    let capture = h.run(one_shot)?;
    h.check(
        "S4 one-shot statement succeeded",
        capture.results.first().map(|r| r.success).unwrap_or(true),
        format!(
            "result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.check(
        "S4 tab override not repinned",
        h.editor.tab_transaction_mode_override_value().is_none(),
        format!(
            "override: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    h.check(
        "S4 no TransactionModeChanged event",
        capture.mode_changes.is_empty(),
        format!("mode change events: {:?}", capture.mode_changes),
    );
    // Oracle: end the transaction SET TRANSACTION opened. MySQL family: a
    // plain ROLLBACK would leave the one-shot override pending (the preflight
    // refuses it); starting the next transaction consumes the override first.
    // Single statements on purpose — a multi-statement script classifies as
    // Script and the preflight will not treat it as a consumer.
    if target.is_oracle() {
        h.run("ROLLBACK")?;
    } else {
        h.run("START TRANSACTION")?;
        h.run("ROLLBACK")?;
    }

    // ---- S5 (MySQL family): toolbar change supersedes a pending one-shot ---
    if !target.is_oracle() {
        println!("  --- S5 toolbar change supersedes a pending one-shot SET TRANSACTION ---");
        h.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")?;
        let replacement = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadWrite,
        );
        h.editor.set_tab_transaction_mode(replacement);
        let (generation, epoch, db_type) = {
            let guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.connection_generation(),
                guard.pool_context_epoch(),
                guard.db_type(),
            )
        };
        let outcome = h.editor.apply_transaction_mode_to_retained_session(
            generation,
            epoch,
            db_type,
            replacement,
            "verify transaction mode",
        );
        h.check(
            "S5 retained replace applied over the pending one-shot",
            matches!(
                outcome,
                space_query::db::RetainedSessionMutationOutcome::Applied
                    | space_query::db::RetainedSessionMutationOutcome::AppliedWithWarning(_)
                    | space_query::db::RetainedSessionMutationOutcome::NoSession
            ),
            format!("outcome: {outcome:?}"),
        );
        // The decisive check: the NEXT transaction must run under the replaced
        // mode, not the stale one-shot the server had pending.
        h.run("START TRANSACTION")?;
        h.run("INSERT INTO SQ_TM_T VALUES (5)")?;
        let isolation = h.select_scalar(
            "SELECT trx_isolation_level FROM information_schema.innodb_trx \
             WHERE trx_mysql_thread_id = CONNECTION_ID()",
        )?;
        h.check(
            "S5 next transaction runs under the replaced mode",
            isolation
                .replace(['-', '_'], " ")
                .eq_ignore_ascii_case("READ COMMITTED"),
            format!("trx_isolation_level = {isolation:?}"),
        );
        h.run("ROLLBACK")?;
        h.editor.clear_tab_transaction_mode_override();
    }

    Ok(())
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
        editor.set_progress_callback(move |event| match progress_inner(&event) {
            QueryProgress::Message { lines, .. } | QueryProgress::ScriptOutput { lines, .. } => {
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
            QueryProgress::TransactionModeChanged { mode } => {
                capture
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .mode_changes
                    .push(*mode);
            }
            QueryProgress::BatchFinished => {
                done.store(true, Ordering::SeqCst);
            }
            _ => {}
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

    h.editor.clear_tab_transaction_mode_override();
    for sql in target.teardown() {
        let _ = h.run(&sql);
    }
    scenario_result?;

    Ok(h.failures)
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
        println!("\nALL TRANSACTION-MODE LIVE CHECKS PASSED");
    } else {
        println!("\nFAILURES:");
        for f in &all_failures {
            println!(" - {f}");
        }
        std::process::exit(1);
    }
}
