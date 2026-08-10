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
//   S14 a READ ONLY pin refuses DDL too, not only DML, and leaves no object.
//   S15 the OTHER branch of the resolver: a tab with no pin runs under the
//       CONNECTION default, and restoring that default lets it write again.
//   S16 (MySQL family) the pinned isolation is behaviourally in force from the
//       FIRST transaction (Oracle gets this from S11), and returning to
//       Default puts the connection's own isolation back on the session.
//   S17 changing the CONNECTION default (what Preferences writes; it bumps the
//       pool-context epoch) leaves a pinned tab pinned and behaving that way,
//       while an unpinned neighbour tab picks the new default up.
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
                "DROP TABLE SQ_TM_ISO".into(),
                "DROP TABLE SQ_TM_TXN".into(),
                "DROP TABLE SQ_TM_DDL".into(),
                "CREATE TABLE SQ_TM_T (V NUMBER)".into(),
                "INSERT INTO SQ_TM_T VALUES (1)".into(),
                "CREATE TABLE SQ_TM_ISO (V NUMBER)".into(),
                "INSERT INTO SQ_TM_ISO VALUES (100)".into(),
                "CREATE TABLE SQ_TM_TXN (V NUMBER)".into(),
                "INSERT INTO SQ_TM_TXN VALUES (0)".into(),
                "COMMIT".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TM_T".into(),
                "DROP TABLE IF EXISTS SQ_TM_ISO".into(),
                "DROP TABLE IF EXISTS SQ_TM_TXN".into(),
                "DROP TABLE IF EXISTS SQ_TM_DDL".into(),
                "CREATE TABLE SQ_TM_T (V INT)".into(),
                "INSERT INTO SQ_TM_T VALUES (1)".into(),
                "CREATE TABLE SQ_TM_ISO (V INT)".into(),
                "INSERT INTO SQ_TM_ISO VALUES (100)".into(),
                "CREATE TABLE SQ_TM_TXN (V INT)".into(),
                "INSERT INTO SQ_TM_TXN VALUES (0)".into(),
                "COMMIT".into(),
            ]
        }
    }

    fn teardown(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                "DROP TABLE SQ_TM_T".into(),
                "DROP TABLE SQ_TM_ISO".into(),
                "DROP TABLE SQ_TM_TXN".into(),
                "DROP TABLE SQ_TM_DDL".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TM_T".into(),
                "DROP TABLE IF EXISTS SQ_TM_ISO".into(),
                "DROP TABLE IF EXISTS SQ_TM_TXN".into(),
                "DROP TABLE IF EXISTS SQ_TM_DDL".into(),
            ]
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

    /// Toolbar Rollback button (async transaction action), pumped to
    /// completion — distinct from a typed ROLLBACK statement.
    fn toolbar_rollback(&mut self) {
        let before = self
            .editor
            .pooled_session_activity_snapshot()
            .map(|s| s.retained_state());
        self.editor.rollback();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
            let now = self
                .editor
                .pooled_session_activity_snapshot()
                .map(|s| s.retained_state());
            if now != before {
                break;
            }
        }
        let drain = Instant::now() + Duration::from_millis(500);
        while Instant::now() < drain {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
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

/// One transaction of the tab under test, reading `SQ_TM_ISO` before and after
/// `other` commits a change to it. A DML on an unrelated table opens the
/// transaction first, so the pair reports the isolation level and never the
/// accident of there being no transaction at all.
fn iso_snapshot_pair(h: &mut Harness, other: &mut Harness) -> Result<(i64, i64), String> {
    h.run("UPDATE SQ_TM_TXN SET V = V + 1")?;
    let first = h.select_scalar("SELECT V FROM SQ_TM_ISO")?;
    other.run("UPDATE SQ_TM_ISO SET V = V + 1")?;
    other.run("COMMIT")?;
    let second = h.select_scalar("SELECT V FROM SQ_TM_ISO")?;
    h.run("ROLLBACK")?;
    let parse = |value: String| {
        value
            .trim()
            .parse::<i64>()
            .map_err(|e| format!("SQ_TM_ISO read {value:?}: {e}"))
    };
    Ok((parse(first)?, parse(second)?))
}

fn run_scenarios(target: Target, h: &mut Harness) -> Result<(), String> {
    let initial_connection_mode = h.connection_transaction_mode();
    // Pin WHICH defense refused the write, per driver, so a regression in one
    // cannot hide behind the other: both Oracle drivers refuse non-queries in
    // the client before they reach the server (the server's ORA-01456 is only
    // the backstop, and it stops applying once the batch's own COMMIT ends the
    // read-only transaction), and the MySQL family reports
    // ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION.
    let read_only_errors: &[&str] = match target {
        Target::OracleOci | Target::OracleThin => &["read-only mode blocks"],
        Target::MySql | Target::MariaDb => &["read only"],
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
        println!("  --- S3 pinned isolation is re-applied on the next execution ---");
        let v = h.select_v()?;
        h.check(
            "S3 SELECT under SERIALIZABLE works",
            v == 1,
            format!("V = {v}"),
        );
        // Oracle has no session view reporting the isolation level, but the
        // application of it is observable: a non-default mode makes the app
        // issue SET TRANSACTION ... at execution start, and that statement
        // itself opens a transaction. So a plain SELECT on a pinned tab must
        // leave the session holding one — evidence the mode really was
        // re-applied rather than silently skipped.
        let after_pinned_select = h
            .editor
            .pooled_session_activity_snapshot()
            .map(|snapshot| snapshot.retained_state());
        h.check(
            "S3 pinned mode was issued (its SET TRANSACTION opened a transaction)",
            after_pinned_select.is_some_and(|state| state.may_have_uncommitted_work()),
            format!("retained state after SELECT on a pinned tab = {after_pinned_select:?}"),
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

    // ---- S6 (Oracle): query, TOOLBAR rollback, then a query-driven
    //      transaction-mode change must not hit ORA-01453 "SET TRANSACTION
    //      must be first statement of transaction" ------------------------------
    if target.is_oracle() {
        println!("  --- S6 query -> toolbar Rollback -> query-driven SET TRANSACTION ---");
        // A non-default tab mode makes the app prepend SET TRANSACTION at every
        // execution start; the SELECT then leaves that transaction open on the
        // retained session. The toolbar Rollback (async action, distinct from a
        // typed ROLLBACK statement) must actually close it, or the next
        // user SET TRANSACTION is not the first statement -> ORA-01453.
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadWrite,
        ));
        h.run("SELECT V FROM SQ_TM_T")?;
        h.toolbar_rollback();
        let capture = h.run("SET TRANSACTION READ ONLY")?;
        let refused = capture
            .results
            .iter()
            .any(|r| !r.success && r.message.to_ascii_uppercase().contains("ORA-01453"));
        h.check(
            "S6 query-driven SET TRANSACTION after toolbar Rollback is not ORA-01453",
            !refused,
            format!(
                "results: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        h.run("ROLLBACK")?;
        h.editor.clear_tab_transaction_mode_override();
    }

    // ---- S10: a READ ONLY pin must survive a COMMIT inside the batch -------
    // The tab setting says "this tab does not write". Oracle can only express
    // it as a TRANSACTION property (SET TRANSACTION READ ONLY), so the user's
    // own COMMIT ends the read-only transaction and everything after it runs
    // read-write unless the client refuses it; the MySQL family applies a
    // SESSION characteristic, which survives the COMMIT by itself. Either way
    // the promise the toolbar makes is the same, so the outcome must be too.
    println!("  --- S10 READ ONLY pin survives a COMMIT inside the batch ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("DELETE FROM SQ_TM_T WHERE V = 77")?;
    h.run("COMMIT")?;
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let capture = h.run("SELECT V FROM SQ_TM_T;\nCOMMIT;\nINSERT INTO SQ_TM_T VALUES (77);")?;
    let insert_after_commit = capture
        .results
        .iter()
        .find(|r| r.sql.to_uppercase().contains("INSERT"))
        .cloned();
    h.check(
        "S10 the write after the batch's own COMMIT is still refused",
        insert_after_commit.as_ref().is_some_and(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "insert after COMMIT: {:?}; all results: {:?}",
            insert_after_commit
                .as_ref()
                .map(|r| (r.success, r.message.clone())),
            capture
                .results
                .iter()
                .map(|r| (r.sql.clone(), r.success, r.message.clone()))
                .collect::<Vec<_>>()
        ),
    );
    // Read back on this same session, so even an uncommitted leak is visible.
    h.editor.clear_tab_transaction_mode_override();
    let leaked = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 77")?;
    h.check(
        "S10 the refused write left no row on the session",
        leaked.trim() == "0",
        format!("COUNT(*) WHERE V = 77 = {leaked:?}"),
    );
    h.run("ROLLBACK")?;

    // ---- S11: selecting Default again must really restore the session ------
    // A session-scoped statement (ALTER SESSION SET ISOLATION_LEVEL / SET
    // SESSION TRANSACTION) is adopted into the tab, so the toolbar shows the
    // new level. Selecting "Default" afterwards must put the SESSION back, not
    // only the label: otherwise the tab keeps running under the old isolation
    // while the screen claims the connection default.
    println!("  --- S11 selecting Default again restores the session isolation ---");
    h.run(session_isolation_sql)?;
    h.run("ROLLBACK")?;
    h.check(
        "S11 the session-scoped change is showing as the tab's mode",
        h.editor
            .tab_transaction_mode_override_value()
            .is_some_and(|mode| mode.isolation == expected_isolation),
        format!(
            "tab override after the change: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    h.editor
        .set_tab_transaction_mode(TransactionMode::default());
    if !target.is_oracle() {
        let isolation_var = if target == Target::MariaDb {
            "SELECT @@tx_isolation"
        } else {
            "SELECT @@transaction_isolation"
        };
        let value = h.select_scalar(isolation_var)?;
        h.check(
            "S11 the session no longer reports the abandoned isolation",
            !value
                .replace(['-', '_'], " ")
                .eq_ignore_ascii_case("SERIALIZABLE"),
            format!("{isolation_var} = {value:?} after returning to Default"),
        );
        h.run("ROLLBACK")?;
    } else {
        // Oracle exposes no session view for the isolation level, so read it
        // behaviourally with a second, independent session: SERIALIZABLE fixes
        // the snapshot for the whole transaction, READ COMMITTED refreshes it
        // per statement. A DML on an unrelated table opens the transaction, so
        // neither half can pass by simply never having one open.
        let mut other = attach_tab(connect_target(target)?);
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            expected_isolation,
            TransactionAccessMode::ReadWrite,
        ));
        let (first, second) = iso_snapshot_pair(h, &mut other)?;
        h.check(
            "S11 SERIALIZABLE really holds the transaction snapshot",
            first == second,
            format!("reads inside one SERIALIZABLE transaction: {first} then {second}"),
        );
        h.editor
            .set_tab_transaction_mode(TransactionMode::default());
        let (third, fourth) = iso_snapshot_pair(h, &mut other)?;
        h.check(
            "S11 back on Default the session reads committed changes again",
            fourth == third + 1,
            format!(
                "reads after returning to Default: {third} then {fourth} \
                 (unchanged means the session is still SERIALIZABLE)"
            ),
        );
    }
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;

    // ---- S12: the pin belongs to ONE tab, not to the connection ------------
    // S1 proves the shared connection's default is untouched. That is a
    // different claim from the one the user reads on screen: another tab of
    // the same connection must keep working normally while this tab is pinned.
    println!("  --- S12 a second tab on the same connection is unaffected ---");
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    h.run("SELECT V FROM SQ_TM_T")?; // applies the pin to this tab's session
    {
        let mut tab2 = attach_tab(Arc::clone(&h.shared));
        let capture = tab2.run("INSERT INTO SQ_TM_T VALUES (12)")?;
        let wrote = capture.results.first().map(|r| r.success).unwrap_or(false);
        h.check(
            "S12 the second tab still writes while this tab is pinned READ ONLY",
            wrote,
            format!(
                "second tab INSERT: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.check(
            "S12 the second tab carries no pinned mode of its own",
            tab2.editor.tab_transaction_mode_override_value().is_none(),
            format!(
                "second tab override: {:?}",
                tab2.editor.tab_transaction_mode_override_value()
            ),
        );
        let _ = tab2.run("ROLLBACK");
    }
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();

    // ---- S13: every combination the toolbar offers must be executable ------
    // The isolation and access-mode choices are independent controls, so the
    // user can select any pair. Oracle cannot express READ ONLY together with
    // an explicit isolation level; a pair that can never run must therefore be
    // refused where it is chosen, not silently pinned onto the tab so that
    // every later statement fails.
    println!("  --- S13 an unsupported isolation/access pair cannot be pinned ---");
    let awkward = TransactionMode::new(
        TransactionIsolation::Serializable,
        TransactionAccessMode::ReadOnly,
    );
    let selection_error = space_query::db::DatabaseConnection::transaction_mode_selection_error(
        h.shared.lock().unwrap_or_else(|p| p.into_inner()).db_type(),
        awkward,
    );
    if target.is_oracle() {
        h.check(
            "S13 Oracle reports the unsupported pair when it is selected",
            selection_error.is_some(),
            "no error was reported for SERIALIZABLE + READ ONLY".into(),
        );
    } else {
        h.check(
            "S13 the MySQL family accepts the pair",
            selection_error.is_none(),
            format!("unexpected error: {selection_error:?}"),
        );
        h.editor.set_tab_transaction_mode(awkward);
        let capture = h.run("SELECT V FROM SQ_TM_T")?;
        h.check(
            "S13 the accepted pair really runs",
            capture.results.iter().all(|r| r.success),
            format!(
                "results: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        h.run("ROLLBACK")?;
        h.editor.clear_tab_transaction_mode_override();
    }

    // ---- S7: the status bar's transaction-state source -----------------------
    // The status bar reports the tab's retained-session state. Prove the source
    // actually carries it: with auto-commit off an UPDATE must leave a retained
    // session that reports uncommitted work (the indicator's "transaction open"
    // case), and a ROLLBACK must clear it again (indicator disappears).
    println!("  --- S7 retained session reports transaction state for the status bar ---");
    h.editor.clear_tab_transaction_mode_override();
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_auto_commit(false)
            .map_err(|e| format!("set_auto_commit(false): {e}"))?;
    }
    h.editor.sync_tab_auto_commit_with_global_setting(false);
    h.run("UPDATE SQ_TM_T SET V = V + 1")?;
    let dirty = h
        .editor
        .pooled_session_activity_snapshot()
        .map(|snapshot| snapshot.retained_state());
    h.check(
        "S7 retained session exists and reports uncommitted work",
        dirty.is_some_and(|state| state.may_have_uncommitted_work()),
        format!("retained state after DML = {dirty:?}"),
    );
    h.run("ROLLBACK")?;
    let after_rollback = h
        .editor
        .pooled_session_activity_snapshot()
        .map(|snapshot| snapshot.retained_state());
    h.check(
        "S7 state clears after ROLLBACK (indicator disappears)",
        after_rollback.is_none_or(|state| {
            !state.may_have_uncommitted_work()
                && !state.requires_transaction_decision()
                && !state.may_have_transaction_mode_override()
        }),
        format!("retained state after ROLLBACK = {after_rollback:?}"),
    );

    // ---- S14: a READ ONLY pin refuses DDL, not only DML --------------------
    // The toolbar promise is "this tab does not write", and CREATE TABLE
    // writes. Oracle expresses read-only per transaction, so its client gate
    // has to refuse the statement itself; the MySQL family relies on the
    // server. Both must reach the same answer, and neither may leave the
    // object behind.
    println!("  --- S14 a READ ONLY pin refuses DDL as well as DML ---");
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let ddl = if target.is_oracle() {
        "CREATE TABLE SQ_TM_DDL (V NUMBER)"
    } else {
        "CREATE TABLE SQ_TM_DDL (V INT)"
    };
    let capture = h.run(ddl)?;
    let ddl_refused = capture.results.first().is_some_and(|r| {
        !r.success
            && read_only_errors.iter().any(|needle| {
                r.message
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
    });
    h.check(
        "S14 CREATE TABLE is refused on the read-only tab",
        ddl_refused,
        format!(
            "ddl result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let ddl_probe = if target.is_oracle() {
        "SELECT COUNT(*) FROM USER_TABLES WHERE TABLE_NAME = 'SQ_TM_DDL'"
    } else {
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'SQ_TM_DDL'"
    };
    let created = h.select_scalar(ddl_probe)?;
    h.check(
        "S14 the refused DDL created nothing",
        created.trim() == "0",
        format!("{ddl_probe} = {created:?}"),
    );
    h.run("ROLLBACK")?;

    // ---- S15: the CONNECTION default drives a tab that pinned nothing ------
    // Every other scenario writes the tab override. The other branch of
    // `effective_transaction_mode` — no override, connection default — is the
    // one a user gets from the connection's advanced settings, and nothing
    // else exercises it.
    println!("  --- S15 the connection default applies to a tab with no pin ---");
    h.editor.clear_tab_transaction_mode_override();
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(TransactionMode::new(
                TransactionIsolation::Default,
                TransactionAccessMode::ReadOnly,
            ))
            .map_err(|e| format!("set connection transaction mode READ ONLY: {e}"))?;
    }
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (15)")?;
    let default_refused = capture.results.first().is_some_and(|r| {
        !r.success
            && read_only_errors.iter().any(|needle| {
                r.message
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
    });
    h.check(
        "S15 the connection default READ ONLY refuses the write with no tab pin",
        default_refused,
        format!(
            "insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.check(
        "S15 no tab override was invented",
        h.editor.tab_transaction_mode_override_value().is_none(),
        format!(
            "override: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    h.run("ROLLBACK")?;
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(initial_connection_mode)
            .map_err(|e| format!("restore connection transaction mode: {e}"))?;
    }
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (15)")?;
    h.check(
        "S15 restoring the connection default lets the same tab write again",
        capture.results.first().map(|r| r.success).unwrap_or(false),
        format!(
            "insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;

    // ---- S16 (MySQL family): the pinned isolation is behaviourally in force
    //      from the FIRST transaction, and the app's own setup statements do
    //      not roll the user's transaction back underneath them ---------------
    // S3 reads @@transaction_isolation, which proves the SET arrived; it does
    // not prove the running transaction obeys it. MySQL fixes isolation at
    // transaction start, so "applied one transaction late" reads correct on
    // the variable and behaves wrong — exactly the failure mode the READ ONLY
    // pin had. Oracle gets this from S11; this is its MySQL-family twin.
    if !target.is_oracle() {
        println!("  --- S16 the pinned isolation really governs the first transaction ---");
        let default_isolation = {
            let guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.default_transaction_isolation()
        };
        let default_holds_snapshot = matches!(
            default_isolation,
            TransactionIsolation::RepeatableRead | TransactionIsolation::Serializable
        );
        let isolation_var = if target == Target::MariaDb {
            "SELECT @@tx_isolation"
        } else {
            "SELECT @@transaction_isolation"
        };
        let mut other = attach_tab(connect_target(target)?);

        // A snapshot-holding level must hold it from the FIRST transaction.
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::RepeatableRead,
            TransactionAccessMode::ReadWrite,
        ));
        let (first, second) = iso_snapshot_pair(h, &mut other)?;
        h.check(
            "S16 a REPEATABLE READ pin holds the snapshot in its first transaction",
            first == second,
            format!(
                "reads inside one REPEATABLE READ transaction: {first} then {second} \
                 (a change means the pin took effect a transaction too late)"
            ),
        );

        // ... and a non-snapshot level must not, on the very next transaction.
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadWrite,
        ));
        let (third, fourth) = iso_snapshot_pair(h, &mut other)?;
        h.check(
            "S16 switching to READ COMMITTED takes effect on the next transaction",
            fourth == third + 1,
            format!(
                "reads after switching to READ COMMITTED: {third} then {fourth} \
                 (unchanged means the session is still REPEATABLE READ)"
            ),
        );

        // Returning to Default must put the CONNECTION's isolation back on the
        // session, not merely on the toolbar label.
        h.editor
            .set_tab_transaction_mode(TransactionMode::default());
        let reported = h.select_scalar(isolation_var)?;
        h.run("ROLLBACK")?;
        h.check(
            "S16 returning to Default puts the connection's own isolation back",
            reported
                .replace(['-', '_'], " ")
                .eq_ignore_ascii_case(&default_isolation.label().replace(['-', '_'], " ")),
            format!(
                "{isolation_var} = {reported:?}, connection default = {:?}",
                default_isolation.label()
            ),
        );
        let (fifth, sixth) = iso_snapshot_pair(h, &mut other)?;
        h.check(
            "S16 on Default the tab behaves like the connection's isolation",
            (fifth == sixth) == default_holds_snapshot,
            format!(
                "reads on Default: {fifth} then {sixth}; connection default = {} \
                 (expected the snapshot to {} held)",
                default_isolation.label(),
                if default_holds_snapshot {
                    "be"
                } else {
                    "not be"
                }
            ),
        );
        h.editor.clear_tab_transaction_mode_override();
        h.run("ROLLBACK")?;
    }

    // ---- S17: changing the CONNECTION default must not disturb a pinned tab
    // The connection default is what Preferences / connection settings write,
    // and `set_transaction_mode` bumps the pool-context epoch — which
    // invalidates retained sessions. A tab that pinned its own mode must come
    // out the other side still pinned and still behaving that way, and a tab
    // that pinned nothing must pick the new default up.
    println!("  --- S17 a connection-default change leaves a pinned tab alone ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    let pin = TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    );
    h.editor.set_tab_transaction_mode(pin);
    h.run("SELECT V FROM SQ_TM_T")?; // apply the pin to this tab's session
    h.run("ROLLBACK")?;
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(TransactionMode::new(
                TransactionIsolation::Serializable,
                TransactionAccessMode::ReadWrite,
            ))
            .map_err(|e| format!("set connection transaction mode SERIALIZABLE: {e}"))?;
    }
    h.check(
        "S17 the tab's pin survived the connection-default change",
        h.editor.tab_transaction_mode_override_value() == Some(pin),
        format!(
            "override after the connection change: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (17)")?;
    let still_read_only = capture.results.first().is_some_and(|r| {
        !r.success
            && read_only_errors.iter().any(|needle| {
                r.message
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
    });
    h.check(
        "S17 the pinned tab still behaves READ ONLY over the new default",
        still_read_only,
        format!(
            "insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    // A neighbour tab that pinned nothing must follow the NEW default instead.
    {
        let mut tab2 = attach_tab(Arc::clone(&h.shared));
        tab2.run("ROLLBACK")?;
        let capture = tab2.run("INSERT INTO SQ_TM_T VALUES (17)")?;
        h.check(
            "S17 an unpinned neighbour tab writes under the new READ WRITE default",
            capture.results.first().map(|r| r.success).unwrap_or(false),
            format!(
                "second tab insert: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let _ = tab2.run("ROLLBACK");
    }
    h.editor.clear_tab_transaction_mode_override();
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(initial_connection_mode)
            .map_err(|e| format!("restore connection transaction mode: {e}"))?;
    }
    h.run("ROLLBACK")?;

    // ---- S9: the toolbar gate opens and closes with the transaction --------
    // The mode controls are disabled exactly when the tab's session cannot take
    // a change. Uncommitted work blocks it on EVERY backend: replacement needs
    // a clean transaction, so the family difference (the MySQL side can replace
    // over a pending one-shot override) only shows on a clean session, which is
    // what S5 covers.
    println!("  --- S9 toolbar gate closes on uncommitted work and reopens after ROLLBACK ---");
    h.run("UPDATE SQ_TM_T SET V = V + 1")?;
    let dirty_decision = h.editor.pooled_session_activity_snapshot().map(|snapshot| {
        space_query::db::retained_session_state_transaction_mode_change_preflight_decision(
            snapshot.db_type,
            snapshot.retained_state(),
        )
    });
    h.check(
        "S9 uncommitted work disables the controls",
        dirty_decision
            == Some(space_query::db::RetainedSessionPreflightDecision::RequireResolution),
        format!("decision while dirty = {dirty_decision:?}"),
    );
    h.run("ROLLBACK")?;
    let clean_decision = h.editor.pooled_session_activity_snapshot().map(|snapshot| {
        space_query::db::retained_session_state_transaction_mode_change_preflight_decision(
            snapshot.db_type,
            snapshot.retained_state(),
        )
    });
    h.check(
        "S9 controls are allowed again once the transaction is resolved",
        clean_decision
            .is_none_or(|d| d == space_query::db::RetainedSessionPreflightDecision::Allow),
        format!("decision after ROLLBACK = {clean_decision:?}"),
    );
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_auto_commit(true)
            .map_err(|e| format!("restore auto_commit: {e}"))?;
    }
    h.editor.sync_tab_auto_commit_with_global_setting(true);

    // ---- S8 (Oracle): the tab's pinned mode survives script CONNECT ---------
    // Runs LAST on purpose: CONNECT rebinds the tab to a transient connection,
    // so the harness's own shared connection no longer drives the tab.
    if target.is_oracle() {
        println!("  --- S8 tab transaction mode survives script CONNECT ---");
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        ));
        // CONNECT refuses to discard a session that may still need resolution
        // — a separate, correct guard. Clear the tab's session first, the way
        // the app tells the user to, so this scenario tests what it means to:
        // whether the tab's pinned mode survives the reconnect.
        let _ = h.editor.discard_pooled_session_for_close();
        let info = target.connection_info();
        let script = format!(
            "CONNECT {}/{}@{}:{}/{}\nINSERT INTO SQ_TM_T VALUES (8);",
            info.username, info.password, info.host, info.port, info.service_name
        );
        let capture = h.run(&script)?;
        let insert = capture
            .results
            .iter()
            .find(|result| result.sql.to_uppercase().contains("INSERT"))
            .cloned();
        // The new connection's own default is Read write. Only the tab pin,
        // re-resolved over that default, can still refuse this write.
        h.check(
            "S8 pinned READ ONLY still refuses the write after CONNECT",
            insert.as_ref().is_some_and(|result| {
                !result.success
                    && read_only_errors.iter().any(|needle| {
                        result
                            .message
                            .to_ascii_lowercase()
                            .contains(&needle.to_ascii_lowercase())
                    })
            }),
            format!(
                "insert after CONNECT: {:?}; all results: {:?}; messages: {:?}",
                insert.map(|r| (r.success, r.message)),
                capture
                    .results
                    .iter()
                    .map(|r| (r.sql.clone(), r.success, r.message.clone()))
                    .collect::<Vec<_>>(),
                capture.messages
            ),
        );
        h.run("ROLLBACK")?;
        h.editor.clear_tab_transaction_mode_override();
    }

    Ok(())
}

/// A second query tab on an existing connection, or (with a connection of its
/// own) a second, independent database session. Both are needed to verify the
/// claims the feature makes about *scope*: a tab-scoped setting must not reach
/// another tab, and an isolation level can only be read behaviourally on
/// Oracle, which needs a session that is not the one under test.
fn attach_tab(shared: space_query::db::SharedConnection) -> Harness {
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
    Harness {
        editor,
        done,
        capture,
        shared,
        failures: Vec::new(),
    }
}

fn connect_target(target: Target) -> Result<space_query::db::SharedConnection, String> {
    let mut connection = DatabaseConnection::new();
    connection
        .connect(target.connection_info())
        .map_err(|e| format!("connect: {e}"))?;
    Ok(Arc::new(Mutex::new(connection)))
}

fn verify(target: Target) -> Result<Vec<String>, String> {
    println!("\n########## {} ##########", target.label());

    let shared = connect_target(target)?;
    let mut h = attach_tab(Arc::clone(&shared));

    for (i, sql) in target.setup().into_iter().enumerate() {
        let r = h.run(&sql);
        if i >= 3 {
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
