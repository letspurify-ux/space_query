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
//   S18 the pinned ISOLATION survives a COMMIT inside the user's own batch
//       (the isolation twin of S10), read behaviourally: two reads in one
//       post-COMMIT transaction with another session committing between them.
//   S19 the other direction of the pin: a READ WRITE tab writes over a READ
//       ONLY connection default, while an unpinned tab is still refused.
//   S20 a locking read keeps its lock until the tab resolves it.
//   S21 a cancelled statement leaves the tab's pin in place, and it still
//       governs the session the tab uses next.
//   S22 the pin survives a disconnect + reconnect and is applied to the new
//       connection's session.
//   S23 an open lazy fetch holds the tab's session, so the transaction-mode
//       controls are closed until it is fetched out or cancelled.
//   S24 a pinned READ ONLY still refuses the write that follows an
//       auto-committed read inside the same batch.
//   S25 (MySQL family) the combined one-statement form adopts BOTH properties,
//       notifies the UI, and both really apply.
//   S30 (MySQL family) the two per-transaction READ WRITE escape forms
//       (one-shot SET TRANSACTION, START TRANSACTION READ WRITE) are refused
//       on a READ ONLY tab; Oracle keeps the same promise via its client gate.
//   S26 the toolbar's OTHER half: picking a mode pins the tab AND applies the
//       change to the tab's retained session. Driven on the state the toolbar
//       meets in practice - a session the tab has already read on, which under
//       manual commit still has a transaction open - in both directions.
//   S27 that same mutation over uncommitted work: it must neither resolve nor
//       discard the user's work.
//   S31 a pin this database cannot express (what a tab bound to another
//       database keeps) must not brick the tab: every isolation/access pair
//       still reads, and still writes exactly when its access mode allows it.
//   S32 the pin survives a change of the tab's scope (the object browser's
//       database/schema selection), which re-applies the session context.
//   S35 (MySQL family) the assignment spellings of the READ WRITE escape
//       (SET @@transaction_read_only = 0; MariaDB SET STATEMENT
//       transaction_read_only=0 FOR <write>) are refused on a READ ONLY tab.
//   S36 (MySQL family) the one-shot assignment spelling
//       (SET @@transaction_isolation = ...) gets the same transaction-boundary
//       prepare as the SET TRANSACTION word form, and stays a one-shot.
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
                "CREATE TABLE SQ_TM_BIG AS SELECT LEVEL AS V FROM DUAL CONNECT BY LEVEL <= 500"
                    .into(),
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
                "CREATE TABLE SQ_TM_BIG (V INT)".into(),
                "INSERT INTO SQ_TM_BIG (V) WITH RECURSIVE seq AS (SELECT 1 AS n UNION ALL \
                 SELECT n + 1 FROM seq WHERE n < 500) SELECT n FROM seq"
                    .into(),
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
                "DROP TABLE SQ_TM_BIG".into(),
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TM_T".into(),
                "DROP TABLE IF EXISTS SQ_TM_ISO".into(),
                "DROP TABLE IF EXISTS SQ_TM_TXN".into(),
                "DROP TABLE IF EXISTS SQ_TM_DDL".into(),
                "DROP TABLE IF EXISTS SQ_TM_BIG".into(),
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

    /// Pumps the event loop for a fixed time without waiting for anything —
    /// used to let a batch that is already running reach a known point (a
    /// sleeping statement) so another session can commit while it is there.
    fn pump_for(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Starts a batch without waiting for it, so the caller can interleave
    /// work from another session while it runs. Pair with `finish_started`.
    fn start(&mut self, sql: &str) {
        self.done.store(false, Ordering::SeqCst);
        *self.capture.lock().unwrap_or_else(|p| p.into_inner()) = RunCapture::default();
        self.editor.execute_sql_text(sql);
    }

    fn finish_started(&mut self) -> Result<RunCapture, String> {
        let done = Arc::clone(&self.done);
        self.pump_until("started batch to finish", || done.load(Ordering::SeqCst))?;
        Ok(std::mem::take(
            &mut *self.capture.lock().unwrap_or_else(|p| p.into_inner()),
        ))
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

    /// Everything the toolbar does when the user picks a mode, in the GUI's
    /// order (`update_transaction_mode_from_controls`): pin the tab, then
    /// apply the change to the tab's retained DB session.
    /// `set_tab_transaction_mode` on its own is only the first half — the
    /// second half is what makes the live session agree with the toolbar
    /// before the next statement runs.
    fn toolbar_transaction_mode(&mut self, mode: TransactionMode) -> String {
        let (db_type, connection_generation, pool_context_epoch) = {
            let guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.db_type(),
                guard.connection_generation(),
                guard.pool_context_epoch(),
            )
        };
        self.editor.set_tab_transaction_mode(mode);
        let outcome = self.editor.apply_transaction_mode_to_retained_session(
            connection_generation,
            pool_context_epoch,
            db_type,
            mode,
            "Updating transaction mode",
        );
        format!("{outcome:?}")
    }

    /// A scope change the way the GUI makes one: the object browser sets the
    /// tab's binding scope AND pushes it onto the tab's retained session
    /// (`synchronize_scope_for_connection` + `apply_retained_scope_update`).
    /// Driving only the binding half would leave a session the tab is already
    /// holding on the old database/schema.
    fn change_tab_scope(&mut self, scope: Option<&str>) -> String {
        self.editor.set_tab_scope(scope.map(str::to_string));
        let (db_type, connection_generation, pool_context_epoch, advanced) = {
            let guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.db_type(),
                guard.connection_generation(),
                guard.pool_context_epoch(),
                guard.get_info().advanced.clone(),
            )
        };
        let outcome = self.editor.apply_current_scope_to_retained_session(
            connection_generation,
            pool_context_epoch,
            db_type,
            scope.unwrap_or(""),
            &advanced,
        );
        format!("{outcome:?}")
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

/// Every value one batch read, and every statement of it that failed.
type BracketedBatch = (Vec<i64>, Vec<(String, String)>);

/// Runs one batch of the tab under test whose last two reads of `SQ_TM_ISO`
/// bracket a committed change from `other`, and returns every value the batch
/// read plus any failed statement. `lead` is script text placed before the
/// bracketing pair (S18 uses it for the batch's own COMMIT). Two reads of one
/// transaction must return the same value under a pinned isolation level.
fn bracketed_reads_in_one_batch(
    h: &mut Harness,
    other: &mut Harness,
    lead: &str,
    sleep_statement: &str,
) -> Result<BracketedBatch, String> {
    let script = format!(
        "{lead}SELECT V FROM SQ_TM_ISO;\n\
         {sleep_statement}\n\
         SELECT V FROM SQ_TM_ISO;"
    );
    h.start(&script);
    // Reach the sleeping statement, so the other session's commit lands
    // between the two bracketing reads.
    h.pump_for(Duration::from_millis(2500));
    other.run("UPDATE SQ_TM_ISO SET V = V + 1")?;
    other.run("COMMIT")?;
    let capture = h.finish_started()?;
    let failed = capture
        .results
        .iter()
        .filter(|r| !r.success)
        .map(|r| (r.sql.clone(), r.message.clone()))
        .collect();
    let reads = capture
        .rows
        .iter()
        .filter_map(|row| row.last())
        .filter_map(|cell| cell.trim().parse::<i64>().ok())
        .collect();
    Ok((reads, failed))
}

fn last_pair(reads: &[i64]) -> Option<(i64, i64)> {
    match reads {
        [.., first, second] => Some((*first, *second)),
        _ => None,
    }
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
    // user can select any pair — and every pair the database can express must
    // really run. Serializable + Read only runs everywhere (on Oracle it IS
    // SET TRANSACTION READ ONLY: one consistent snapshot, writes forbidden).
    // Oracle alone cannot run a READ COMMITTED read-only transaction; that
    // pair must be refused where it is chosen, not silently pinned onto the
    // tab so that every later statement fails.
    println!("  --- S13 an unsupported isolation/access pair cannot be pinned ---");
    let serializable_read_only = TransactionMode::new(
        TransactionIsolation::Serializable,
        TransactionAccessMode::ReadOnly,
    );
    let db_type = h.shared.lock().unwrap_or_else(|p| p.into_inner()).db_type();
    let selection_error = space_query::db::DatabaseConnection::transaction_mode_selection_error(
        db_type,
        serializable_read_only,
    );
    h.check(
        "S13 Serializable + Read only is accepted everywhere",
        selection_error.is_none(),
        format!("unexpected error: {selection_error:?}"),
    );
    h.editor.set_tab_transaction_mode(serializable_read_only);
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
    if target.is_oracle() {
        let unrunnable = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );
        let unrunnable_error =
            space_query::db::DatabaseConnection::transaction_mode_selection_error(
                db_type, unrunnable,
            );
        h.check(
            "S13 Oracle reports the read-committed read-only pair when it is selected",
            unrunnable_error.is_some(),
            "no error was reported for READ COMMITTED + READ ONLY".into(),
        );
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

    // ---- S18: the pinned ISOLATION must survive the batch's own COMMIT -----
    // The isolation twin of S10. The tab setting says "this tab runs
    // SERIALIZABLE / REPEATABLE READ", and the promise cannot end halfway
    // through a script: Oracle expresses isolation as a TRANSACTION property
    // (SET TRANSACTION ISOLATION LEVEL ...), so a COMMIT inside the user's own
    // batch ends it and everything after would run at the session default;
    // the MySQL family applies a SESSION characteristic that survives by
    // itself. Read behaviourally, because that is the only honest reading: two
    // reads inside ONE transaction, with another session committing between
    // them, must return the same value.
    println!("  --- S18 the pinned isolation survives a COMMIT inside the batch ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    {
        let mut other = attach_tab(connect_target(target)?);
        let pinned_isolation = if target.is_oracle() {
            TransactionIsolation::Serializable
        } else {
            // REPEATABLE READ, not SERIALIZABLE: InnoDB's SERIALIZABLE turns
            // plain reads into locking reads, which would block the other
            // session's UPDATE instead of testing the snapshot.
            TransactionIsolation::RepeatableRead
        };
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            pinned_isolation,
            TransactionAccessMode::ReadWrite,
        ));
        let sleep_statement = if target.is_oracle() {
            "BEGIN DBMS_SESSION.SLEEP(6); END;\n/"
        } else {
            "DO SLEEP(6);"
        };

        // S18a: the baseline claim, with no COMMIT anywhere in the batch — two
        // reads of one transaction must not straddle another session's commit.
        let (reads, failed) = bracketed_reads_in_one_batch(h, &mut other, "", sleep_statement)?;
        h.check(
            "S18a every statement of the plain batch ran",
            failed.is_empty(),
            format!("failed statements: {failed:?}"),
        );
        let plain_pair = last_pair(&reads);
        h.check(
            "S18a two reads of one batch see one transaction snapshot",
            plain_pair.is_some_and(|(first, second)| first == second),
            format!(
                "reads in the batch: {reads:?} (different values mean the tab's \
                 pinned isolation did not hold across the batch's statements)"
            ),
        );
        h.run("ROLLBACK")?;
        let _ = h.editor.discard_pooled_session_for_close();

        // S18b: the same claim after the user's own COMMIT inside the batch.
        let (reads, failed) = bracketed_reads_in_one_batch(
            h,
            &mut other,
            "SELECT V FROM SQ_TM_ISO;\nCOMMIT;\n",
            sleep_statement,
        )?;
        h.check(
            "S18b every statement of the committing batch ran",
            failed.is_empty(),
            format!("failed statements: {failed:?}"),
        );
        let after_commit_pair = last_pair(&reads);
        h.check(
            "S18b both reads after the batch's COMMIT see one transaction snapshot",
            after_commit_pair.is_some_and(|(first, second)| first == second),
            format!(
                "reads in the batch: {reads:?} (the last two bracket the other \
                 session's commit; different values mean the pinned isolation \
                 was dropped by the COMMIT)"
            ),
        );
        h.run("ROLLBACK")?;
        // Control: the other session really did commit, so the checks above
        // cannot pass by nothing having happened.
        h.editor.clear_tab_transaction_mode_override();
        let now = h.select_scalar("SELECT V FROM SQ_TM_ISO")?;
        h.check(
            "S18 the other session's commits are visible to a new transaction",
            now.trim()
                .parse::<i64>()
                .ok()
                .zip(after_commit_pair)
                .is_some_and(|(now, (first, _))| now > first),
            format!("SQ_TM_ISO after the batch = {now:?}, in-batch reads {reads:?}"),
        );
        let _ = other.run("ROLLBACK");
        let _ = other.editor.discard_pooled_session_for_close();
    }
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    // The sleeping statement is a PL/SQL block (Oracle) / an unclassified
    // command (MySQL family), so the app records conservative session residue
    // for it — correct behaviour, but it would block the next scenario's
    // execution. Discard the session the way the tab's close path does.
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S19: the OTHER direction of the pin — READ WRITE over a READ ONLY
    // connection default. S15 covers a tab with no pin following a READ ONLY
    // default; the tab pin has to be able to lift it again, or a connection
    // configured read-only by default would have no per-tab escape at all.
    println!("  --- S19 a READ WRITE pin lifts a READ ONLY connection default ---");
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(TransactionMode::new(
                TransactionIsolation::Default,
                TransactionAccessMode::ReadOnly,
            ))
            .map_err(|e| format!("set connection default READ ONLY: {e}"))?;
    }
    {
        // Control: with the new default really in force, a tab that pins
        // nothing is refused.
        let mut tab2 = attach_tab(Arc::clone(&h.shared));
        let capture = tab2.run("INSERT INTO SQ_TM_T VALUES (19)")?;
        let refused = capture.results.first().is_some_and(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        });
        h.check(
            "S19 an unpinned tab follows the READ ONLY connection default",
            refused,
            format!(
                "unpinned insert: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let _ = tab2.run("ROLLBACK");
        let _ = tab2.editor.discard_pooled_session_for_close();
    }
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadWrite,
    ));
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (19)")?;
    h.check(
        "S19 the READ WRITE pin writes over the read-only default",
        capture.results.first().map(|r| r.success).unwrap_or(false),
        format!(
            "pinned insert: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .set_transaction_mode(initial_connection_mode)
            .map_err(|e| format!("restore connection transaction mode: {e}"))?;
    }
    h.run("ROLLBACK")?;

    // ---- S20: a locking read must keep its lock until the user resolves it -
    // `SELECT ... FOR UPDATE` is the strongest statement of intent a read can
    // make: the rows are held for the statement the user is about to write.
    // The lock lives on the transaction, so it survives only as long as the
    // tab's transaction does — which makes this the sharpest test of whether
    // the app leaves the tab's transaction alone between executions.
    println!("  --- S20 a locking read keeps its lock until the tab resolves it ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    {
        let mut other = attach_tab(connect_target(target)?);
        let capture = h.run("SELECT V FROM SQ_TM_T FOR UPDATE")?;
        h.check(
            "S20 the locking read ran",
            capture
                .results
                .iter()
                .all(|r| r.success || r.message.is_empty()),
            format!(
                "locking read: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        let capture = other.run("SELECT V FROM SQ_TM_T FOR UPDATE NOWAIT")?;
        let blocked = capture.results.iter().any(|r| !r.success)
            || capture.messages.iter().any(|m| m.contains("Error"));
        h.check(
            "S20 another session cannot take the same rows",
            blocked,
            format!(
                "the other session's locking read: {:?} (success means the tab's \
                 transaction — and with it the lock — was already gone)",
                capture
                    .results
                    .iter()
                    .map(|r| (r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        // The refused locking read leaves the other session in a state the app
        // conservatively wants resolved (MariaDB reports a lock error that a
        // ROLLBACK statement is not allowed to answer on its own), so end that
        // session the way the close path does instead of typing ROLLBACK into
        // it — otherwise the harness meets the resolution dialog and blocks.
        let _ = other.editor.discard_pooled_session_for_close();
        h.run("ROLLBACK")?;
    }
    // A locking read is conservative session state on the MySQL family, and a
    // ROLLBACK does not clear residue — leave the tab a clean session instead
    // of letting the next scenario meet the resolution dialog.
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S21: a cancelled statement must not lose the tab's pin ------------
    // Cancelling stops a statement; it is not a change of what the tab is set
    // to. The pin has to survive it and to reach the session the tab uses next
    // — which is usually a NEW one, because a cancelled statement leaves a
    // session the app tells the user to resolve or discard.
    println!("  --- S21 the tab's pin survives a cancelled statement ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    let pin = TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    );
    h.editor.set_tab_transaction_mode(pin);
    // A long SELECT: a read is what a Read only tab is allowed to run, and it
    // gives the cancel something to interrupt.
    let long_select = if target.is_oracle() {
        "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c"
    } else {
        "SELECT SLEEP(20)"
    };
    h.start(long_select);
    h.pump_for(Duration::from_millis(1500));
    h.editor.cancel_current();
    let capture = h.finish_started()?;
    h.check(
        "S21 the cancelled statement did not succeed",
        capture.results.iter().all(|r| !r.success),
        format!(
            "results: {:?}",
            capture
                .results
                .iter()
                .map(|r| (r.success, r.message.clone()))
                .collect::<Vec<_>>()
        ),
    );
    h.check(
        "S21 the tab is still pinned after the cancel",
        h.editor.tab_transaction_mode_override_value() == Some(pin),
        format!(
            "override after cancel: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    // Resolve the cancelled session the way the app asks the user to, then
    // prove the pin still governs what the tab runs next.
    let _ = h.editor.discard_pooled_session_for_close();
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (21)")?;
    h.check(
        "S21 the pin still refuses the write on the tab's next session",
        capture.results.first().is_some_and(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "insert after cancel: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S22: the pin survives a disconnect and reconnect ------------------
    // Reconnecting replaces every physical session and bumps the connection
    // generation, so the tab's pin has to be re-applied to a session it has
    // never seen. The setting belongs to the tab, not to the session it was
    // last applied to.
    println!("  --- S22 the tab's pin survives a reconnect ---");
    h.editor.set_tab_transaction_mode(pin);
    {
        let mut guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard.disconnect();
        guard
            .connect(target.connection_info())
            .map_err(|e| format!("reconnect: {e}"))?;
    }
    h.check(
        "S22 the tab kept its pin across the reconnect",
        h.editor.tab_transaction_mode_override_value() == Some(pin),
        format!(
            "override after reconnect: {:?}",
            h.editor.tab_transaction_mode_override_value()
        ),
    );
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (22)")?;
    h.check(
        "S22 the pin is applied to the new connection's session",
        capture.results.first().is_some_and(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "insert after reconnect: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (22)")?;
    h.check(
        "S22 the same tab writes again once unpinned",
        capture.results.first().map(|r| r.success).unwrap_or(false),
        format!(
            "unpinned insert after reconnect: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;

    // ---- S23: an open lazy fetch holds the tab's session, so the mode
    // controls must be closed until it is finished or cancelled -------------
    println!("  --- S23 an open lazy fetch blocks a transaction-mode change ---");
    let db_type = {
        let guard = h.shared.lock().unwrap_or_else(|p| p.into_inner());
        guard.db_type()
    };
    h.check(
        "S23 the gate is open on an idle tab",
        !h.editor.transaction_mode_change_blocked_now(db_type),
        "the mode controls are already blocked before the lazy fetch".into(),
    );
    h.run("SELECT V FROM SQ_TM_BIG")?;
    let lazy_session = h.editor.active_lazy_fetch_session();
    h.check(
        "S23 the big SELECT left a lazy fetch open",
        h.editor.has_open_lazy_fetch() && lazy_session.is_some(),
        format!("active lazy fetch session: {lazy_session:?}"),
    );
    h.check(
        "S23 the open lazy fetch blocks a transaction-mode change",
        h.editor.transaction_mode_change_blocked_now(db_type),
        "the mode controls stayed open while a lazy fetch held the session".into(),
    );
    if let Some(session_id) = lazy_session {
        h.editor.request_lazy_fetch(
            session_id,
            space_query::ui::sql_editor::LazyFetchRequest::Cancel,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.editor.has_open_lazy_fetch() && Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    h.check(
        "S23 cancelling the lazy fetch closes it",
        !h.editor.has_open_lazy_fetch(),
        "the lazy fetch is still open after Cancel".into(),
    );
    h.run("ROLLBACK")?;
    h.check(
        "S23 the gate reopens once the lazy fetch is gone",
        !h.editor.transaction_mode_change_blocked_now(db_type),
        format!(
            "still blocked: retained state = {:?}",
            h.editor
                .pooled_session_activity_snapshot()
                .map(|s| s.retained_state())
        ),
    );
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S24: a pinned mode holds across an auto-committed statement -------
    // Auto-commit ends the transaction after every statement, and on Oracle
    // the access mode IS a transaction property — so a pinned READ ONLY tab
    // with auto-commit ON has to re-establish the pin for the statement that
    // follows an auto-commit inside the same batch. The two features are
    // pinned per tab independently; this is where they meet.
    println!("  --- S24 a pinned READ ONLY holds with auto-commit ON ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    h.editor.set_tab_auto_commit(true);
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let capture = h.run("SELECT V FROM SQ_TM_T;\nINSERT INTO SQ_TM_T VALUES (24);")?;
    let select_ok = capture
        .results
        .iter()
        .find(|r| r.is_select)
        .map(|r| r.success)
        .unwrap_or(false);
    h.check(
        "S24 the read of the read-only tab ran",
        select_ok,
        format!(
            "results: {:?}",
            capture
                .results
                .iter()
                .map(|r| (r.success, r.message.clone()))
                .collect::<Vec<_>>()
        ),
    );
    let insert_result = capture.results.iter().find(|r| r.sql.contains("INSERT"));
    h.check(
        "S24 the write after the auto-committed read is still refused",
        insert_result.is_some_and(|r| {
            !r.success
                && read_only_errors.iter().any(|needle| {
                    r.message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "insert result: {:?}",
            insert_result.map(|r| (r.success, r.message.clone()))
        ),
    );
    h.editor.clear_tab_transaction_mode_override();
    h.editor.sync_tab_auto_commit_with_global_setting(false);
    let _ = h.editor.discard_pooled_session_for_close();
    let leaked = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 24")?;
    h.check(
        "S24 the refused write left no row",
        leaked.trim() == "0",
        format!("COUNT(*) WHERE V = 24 = {leaked}"),
    );
    h.run("ROLLBACK")?;

    // ---- S25: one statement that changes BOTH properties at once ----------
    // (MySQL family) `SET SESSION TRANSACTION ISOLATION LEVEL x, READ ONLY`
    // is a single statement carrying both halves of the mode. Adoption must
    // take both, not the first one it recognises.
    if !target.is_oracle() {
        println!("  --- S25 a combined session characteristics change adopts both ---");
        h.editor.clear_tab_transaction_mode_override();
        h.run("ROLLBACK")?;
        let combined = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        let capture = h.run("SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ ONLY")?;
        h.check(
            "S25 the combined statement succeeded",
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
        h.check(
            "S25 the tab adopted both properties",
            h.editor.tab_transaction_mode_override_value() == Some(combined),
            format!(
                "override = {:?}",
                h.editor.tab_transaction_mode_override_value()
            ),
        );
        h.check(
            "S25 the UI was notified of the combined mode",
            capture.mode_changes.last() == Some(&combined),
            format!("TransactionModeChanged events: {:?}", capture.mode_changes),
        );
        let isolation_var = if target == Target::MariaDb {
            "SELECT @@tx_isolation"
        } else {
            "SELECT @@transaction_isolation"
        };
        let isolation = h.select_scalar(isolation_var)?;
        h.check(
            "S25 the adopted isolation is really on the session",
            isolation.to_ascii_uppercase().contains("SERIALIZABLE"),
            format!("{isolation_var} = {isolation}"),
        );
        let capture = h.run("INSERT INTO SQ_TM_T VALUES (25)")?;
        h.check(
            "S25 the adopted READ ONLY really refuses a write",
            capture.results.first().is_some_and(|r| {
                !r.success
                    && read_only_errors.iter().any(|needle| {
                        r.message
                            .to_ascii_lowercase()
                            .contains(&needle.to_ascii_lowercase())
                    })
            }),
            format!(
                "insert result: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.editor.clear_tab_transaction_mode_override();
        let _ = h.editor.discard_pooled_session_for_close();
        h.run("ROLLBACK")?;
    }

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
    // ---- S26: the half of the toolbar write path nothing else drives ------
    // The GUI does two things when a mode is picked: it pins the tab AND it
    // applies the change to the tab's retained DB session. Every scenario
    // above drives only the pin, so the session mutation - the half that runs
    // against a session the tab has already used - was never exercised live.
    // Drive it the way the toolbar does, on the state the toolbar meets in
    // practice: a tab that has just read a table, which under manual commit
    // leaves a transaction open on its session. Whatever the mutation reports,
    // the tab must never write while it shows Read only.
    println!("  --- S26 the toolbar write path reaches the tab's live session ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    let before_rows = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T")?;
    let pin_outcome = h.toolbar_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (26)")?;
    let refused = capture.results.first().is_some_and(|r| {
        !r.success
            && read_only_errors.iter().any(|needle| {
                r.message
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
    });
    h.check(
        "S26 the write after the toolbar pin is refused",
        refused,
        format!(
            "retained-session mutation: {pin_outcome}; insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    let after_rows = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T")?;
    h.check(
        "S26 the refused write left no row behind",
        after_rows == before_rows,
        format!("row count {before_rows} -> {after_rows}"),
    );
    // The same path in the release direction: picking the connection default
    // again has to hand the session back to read-write. A mode change is only
    // taken at a transaction boundary, and on Oracle the read above is inside
    // the transaction the app's own SET TRANSACTION opened - so end it first,
    // exactly as the app tells the user to.
    h.run("ROLLBACK")?;
    let release_outcome = h.toolbar_transaction_mode(TransactionMode::default());
    let capture = h.run("INSERT INTO SQ_TM_T VALUES (27)")?;
    h.check(
        "S26 the toolbar releases the pin on the same session",
        capture.results.first().is_some_and(|r| r.success),
        format!(
            "retained-session mutation: {release_outcome}; insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();

    // ---- S27: the mutation's own gate over the user's work -----------------
    // The toolbar checks the tab's session before it changes anything, but the
    // mutation is a second, independent defense: it must not resolve, discard
    // or silently keep running over work the user has not committed.
    println!("  --- S27 the toolbar mutation leaves uncommitted work alone ---");
    let baseline_v = h.select_v()?;
    h.run("UPDATE SQ_TM_T SET V = V + 1")?;
    let dirty_outcome = h.toolbar_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let still_dirty = h
        .editor
        .pooled_session_activity_snapshot()
        .map(|snapshot| snapshot.retained_state());
    h.check(
        "S27 the work is still uncommitted after the attempt",
        still_dirty.is_some_and(|state| state.may_have_uncommitted_work()),
        format!("mutation: {dirty_outcome}; retained state = {still_dirty:?}"),
    );
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    let restored_v = h.select_v()?;
    h.check(
        "S27 the work is still rollback-able after the attempt",
        restored_v == baseline_v,
        format!("V {baseline_v} -> {restored_v} (mutation: {dirty_outcome})"),
    );

    // ---- S28: every isolation level the toolbar offers really applies ------
    // The scenarios above pin one level each (SERIALIZABLE, REPEATABLE READ).
    // A level the toolbar offers but nothing ever exercises could be spelled
    // wrong, be rejected by the server, or land on the session as a different
    // level, and the app would only find out in a user's hands. Walk every
    // level each backend offers, through the toolbar's own write path, and
    // read the result back where a level is observable.
    println!("  --- S28 every offered isolation level really applies ---");
    {
        let mut other = attach_tab(connect_target(target)?);
        let isolation_variable = match target {
            Target::MariaDb => Some("SELECT @@tx_isolation"),
            Target::MySql => Some("SELECT @@transaction_isolation"),
            Target::OracleOci | Target::OracleThin => None,
        };
        for isolation in offered_isolations(target) {
            h.editor.clear_tab_transaction_mode_override();
            h.run("ROLLBACK")?;
            let _ = h.editor.discard_pooled_session_for_close();
            let outcome = h.toolbar_transaction_mode(TransactionMode::new(
                isolation,
                TransactionAccessMode::ReadWrite,
            ));
            let label = isolation.label();

            if let Some(sql) = isolation_variable {
                let applied = h.select_scalar(sql)?;
                h.check(
                    &format!("S28 {label} is on the session"),
                    isolation_value_matches(&applied, isolation),
                    format!("session reported {applied:?} (retained outcome: {outcome})"),
                );
            }

            match isolation {
                // The only level with a behaviour of its own that no other
                // level shows: a read of another session's uncommitted work.
                TransactionIsolation::ReadUncommitted => {
                    other.run("UPDATE SQ_TM_ISO SET V = V + 1000")?;
                    let seen = h.select_scalar("SELECT V FROM SQ_TM_ISO")?;
                    let uncommitted = other.select_scalar("SELECT V FROM SQ_TM_ISO")?;
                    other.run("ROLLBACK")?;
                    h.run("ROLLBACK")?;
                    h.check(
                        "S28 READ UNCOMMITTED really reads uncommitted work",
                        seen.trim() == uncommitted.trim(),
                        format!(
                            "the tab read {seen:?} while the other session held {uncommitted:?} \
                             uncommitted"
                        ),
                    );
                }
                // Sees another session's commit inside its own transaction.
                TransactionIsolation::ReadCommitted => {
                    let (first, second) = iso_snapshot_pair(h, &mut other)?;
                    h.run("ROLLBACK")?;
                    h.check(
                        "S28 READ COMMITTED sees a commit inside its transaction",
                        second > first,
                        format!("reads bracketing the other session's commit: {first} -> {second}"),
                    );
                }
                // Must not: one transaction, one snapshot.
                TransactionIsolation::RepeatableRead => {
                    let (first, second) = iso_snapshot_pair(h, &mut other)?;
                    h.run("ROLLBACK")?;
                    h.check(
                        "S28 REPEATABLE READ holds one snapshot",
                        first == second,
                        format!("reads bracketing the other session's commit: {first} -> {second}"),
                    );
                }
                TransactionIsolation::Serializable => {
                    if target.is_oracle() {
                        let (first, second) = iso_snapshot_pair(h, &mut other)?;
                        h.run("ROLLBACK")?;
                        h.check(
                            "S28 SERIALIZABLE holds one snapshot",
                            first == second,
                            format!(
                                "reads bracketing the other session's commit: {first} -> {second}"
                            ),
                        );
                    }
                    // InnoDB's SERIALIZABLE turns plain reads into locking
                    // reads, so the same pair would block the other session
                    // instead of reporting a snapshot (see S18). The session
                    // read-back above is the honest check there.
                }
                TransactionIsolation::Default => {}
            }
        }
        h.editor.clear_tab_transaction_mode_override();
        h.run("ROLLBACK")?;
        let _ = h.editor.discard_pooled_session_for_close();
    }

    // ---- S29: a READ ONLY pin refuses a LOCKING read -----------------------
    // The boundary of the read/write classification: `SELECT ... FOR UPDATE`
    // reads like a query and writes like a lock. Oracle's client gate lets it
    // past as a select and the server has to refuse it (ORA-01456); the MySQL
    // family refuses it as a write. Either way a READ ONLY tab must not take
    // a lock, and the same statement must run once the pin is gone — otherwise
    // the check would pass on a statement that simply never works.
    println!("  --- S29 a READ ONLY pin refuses a locking read ---");
    h.toolbar_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let locking_read = "SELECT V FROM SQ_TM_T FOR UPDATE";
    // A locking read is the one statement the Oracle client gate passes on
    // (it reads like a query), so there the server's ORA-01456 is the refusal
    // — the backstop doing its job, which is what this scenario is here for.
    let locking_read_errors: &[&str] = match target {
        Target::OracleOci | Target::OracleThin => &["read-only mode blocks", "ora-01456"],
        Target::MySql | Target::MariaDb => &["read only"],
    };
    let capture = h.run(locking_read)?;
    let refused = capture.results.first().cloned();
    h.check(
        "S29 the locking read is refused while the tab is READ ONLY",
        refused.as_ref().is_some_and(|result| {
            !result.success
                && locking_read_errors.iter().any(|needle| {
                    result
                        .message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "locking read: {:?}; messages: {:?}",
            refused.map(|r| (r.success, r.message)),
            capture.messages
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let _ = h.editor.discard_pooled_session_for_close();
    let capture = h.run(locking_read)?;
    h.check(
        "S29 the same locking read runs once the pin is gone",
        capture.results.first().is_some_and(|result| result.success),
        format!(
            "locking read after unpinning: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;

    // ---- S30: explicit READ WRITE escapes cannot write on a READ ONLY tab --
    // The MySQL family expresses the pin as a SESSION characteristic, and the
    // server itself lets two per-transaction forms override it: a one-shot
    // `SET TRANSACTION READ WRITE` (consumed by the next transaction) and
    // `START TRANSACTION READ WRITE`. Without a client-side gate an INSERT
    // rides them through while the toolbar reads READ ONLY. Oracle's client
    // gate already keeps this promise: the escape statement runs, but the
    // write after it is refused.
    println!("  --- S30 explicit READ WRITE escapes cannot write on a READ ONLY tab ---");
    h.toolbar_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    if target.is_oracle() {
        let capture = h.run("SET TRANSACTION READ WRITE;\nINSERT INTO SQ_TM_T VALUES (30);")?;
        let insert_result = capture
            .results
            .iter()
            .find(|result| result.sql.contains("INSERT"))
            .cloned();
        h.check(
            "S30 the write after a one-shot READ WRITE is still refused",
            insert_result.as_ref().is_some_and(|result| {
                !result.success
                    && read_only_errors.iter().any(|needle| {
                        result
                            .message
                            .to_ascii_lowercase()
                            .contains(&needle.to_ascii_lowercase())
                    })
            }),
            format!(
                "insert result: {:?}",
                insert_result.map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("ROLLBACK")?;
    } else {
        let capture = h.run("SET TRANSACTION READ WRITE")?;
        h.check(
            "S30 the one-shot READ WRITE escape is refused on a READ ONLY tab",
            capture
                .results
                .first()
                .is_some_and(|result| !result.success),
            format!(
                "one-shot escape: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let capture = h.run("INSERT INTO SQ_TM_T VALUES (30)")?;
        h.check(
            "S30 the write after the one-shot attempt is refused",
            capture
                .results
                .first()
                .is_some_and(|result| !result.success),
            format!(
                "insert after one-shot: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let capture =
            h.run("START TRANSACTION READ WRITE;\nINSERT INTO SQ_TM_T VALUES (30);\nCOMMIT;")?;
        h.check(
            "S30 START TRANSACTION READ WRITE is refused on a READ ONLY tab",
            capture
                .results
                .first()
                .is_some_and(|result| !result.success),
            format!(
                "start-transaction escape: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let _ = h.run("ROLLBACK");
    }
    h.editor.clear_tab_transaction_mode_override();
    let _ = h.editor.discard_pooled_session_for_close();
    let leaked = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 30")?;
    h.check(
        "S30 no row landed through an explicit READ WRITE escape",
        leaked.trim() == "0",
        format!("COUNT(*) WHERE V = 30 = {leaked}"),
    );
    h.run("ROLLBACK")?;

    // ---- S31: a pin this database cannot express ---------------------------
    // A tab keeps its pinned mode when it is bound to another database: a tab
    // whose connection went away is bound to the one selected in the object
    // browser on its next execution, and the pin is not part of what that
    // rebinding resets. The isolation catalogs differ per family, so a MySQL
    // tab pinned to Repeatable read can end up carrying a mode Oracle has no
    // statement for. Running it anyway fails EVERY statement on that tab
    // ("Oracle does not support ..."), while the toolbar — whose list only
    // holds this database's levels — shows Default and cannot even send a
    // change event to clear it: a tab the user cannot repair. So whatever a
    // tab ends up pinned to, what it really runs must be a mode this database
    // can express, the tab must keep working, and a READ ONLY pin — the one
    // half every family can express — must still be kept.
    println!("  --- S31 a pin this database cannot express does not brick the tab ---");
    for isolation in [
        TransactionIsolation::Default,
        TransactionIsolation::ReadUncommitted,
        TransactionIsolation::ReadCommitted,
        TransactionIsolation::RepeatableRead,
        TransactionIsolation::Serializable,
    ] {
        for access in [
            TransactionAccessMode::ReadWrite,
            TransactionAccessMode::ReadOnly,
        ] {
            h.editor.clear_tab_transaction_mode_override();
            h.run("ROLLBACK")?;
            let _ = h.editor.discard_pooled_session_for_close();
            let pinned = TransactionMode::new(isolation, access);
            let foreign = DatabaseConnection::transaction_mode_selection_error(db_type, pinned)
                .map_or_else(String::new, |error| {
                    format!(" (not expressible here: {error})")
                });
            h.editor.set_tab_transaction_mode(pinned);
            let label = format!("{} + {}", isolation.label(), access.label());
            let capture = h.run("SELECT V FROM SQ_TM_T")?;
            h.check(
                &format!("S31 the tab still reads while pinned to {label}"),
                capture.results.first().is_some_and(|result| result.success),
                format!(
                    "select result: {:?}{foreign}",
                    capture
                        .results
                        .first()
                        .map(|r| (r.success, r.message.clone()))
                ),
            );
            let capture = h.run("INSERT INTO SQ_TM_T VALUES (31)")?;
            let wrote = capture.results.first().is_some_and(|result| result.success);
            let expected_write = access == TransactionAccessMode::ReadWrite;
            h.check(
                &format!("S31 pinned to {label} the tab writes exactly when it may"),
                wrote == expected_write,
                format!(
                    "write allowed = {wrote}, expected {expected_write}; result: {:?}{foreign}",
                    capture
                        .results
                        .first()
                        .map(|r| (r.success, r.message.clone()))
                ),
            );
            h.run("ROLLBACK")?;
        }
    }
    h.editor.clear_tab_transaction_mode_override();
    let _ = h.editor.discard_pooled_session_for_close();
    let leaked = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 31")?;
    h.check(
        "S31 no row survived the pinned writes",
        leaked.trim() == "0",
        format!("COUNT(*) WHERE V = 31 = {leaked}"),
    );
    h.run("ROLLBACK")?;

    // ---- S32: the pin survives a change of the tab's scope ------------------
    // Selecting another database/schema in the object browser re-applies the
    // tab's session context before the next statement runs, and that path has
    // already overwritten the tab's mode once (the pre-action scope recheck
    // used to re-apply the CONNECTION default). Change the scope under a pin
    // and the pin must still be the thing that governs the next statement —
    // and the release must still work in the new scope, so the refusal cannot
    // be the scope's own doing.
    println!("  --- S32 the tab's pinned mode survives a scope change ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    let base_scope = base_scope(target);
    let qualified_table = format!("{base_scope}.SQ_TM_T");
    let scratch_scope = "SQ_TM_SCOPE";
    let create_scope = if target.is_oracle() {
        format!("CREATE USER {scratch_scope} IDENTIFIED BY pw1")
    } else {
        format!("CREATE DATABASE {scratch_scope}")
    };
    let _ = h.run(&if target.is_oracle() {
        format!("DROP USER {scratch_scope} CASCADE")
    } else {
        format!("DROP DATABASE IF EXISTS {scratch_scope}")
    });
    let capture = h.run(&create_scope)?;
    if !capture.results.first().is_some_and(|result| result.success) {
        return Err(format!(
            "S32 could not create the scratch scope: {:?}",
            capture.results.first().map(|r| r.message.clone())
        ));
    }
    h.toolbar_transaction_mode(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ));
    let scope_outcome = h.change_tab_scope(Some(scratch_scope));
    let current_scope_sql = if target.is_oracle() {
        "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM DUAL"
    } else {
        "SELECT DATABASE()"
    };
    let scope_now = h.select_scalar(current_scope_sql)?;
    h.check(
        "S32 the tab's session really moved to the new scope",
        scope_now.trim().eq_ignore_ascii_case(scratch_scope),
        format!("{current_scope_sql} = {scope_now:?} (scope change: {scope_outcome})"),
    );
    if !target.is_oracle() {
        let read_only_var = if target == Target::MariaDb {
            "SELECT @@tx_read_only"
        } else {
            "SELECT @@transaction_read_only"
        };
        let value = h.select_scalar(read_only_var)?;
        h.check(
            "S32 the session still carries the pin after the scope change",
            matches!(value.trim(), "1" | "ON"),
            format!("{read_only_var} = {value:?}"),
        );
    }
    let capture = h.run(&format!("INSERT INTO {qualified_table} VALUES (32)"))?;
    h.check(
        "S32 the write in the new scope is still refused",
        capture.results.first().is_some_and(|result| {
            !result.success
                && read_only_errors.iter().any(|needle| {
                    result
                        .message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
        }),
        format!(
            "insert in the new scope: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.editor.clear_tab_transaction_mode_override();
    let _ = h.editor.discard_pooled_session_for_close();
    let capture = h.run(&format!("INSERT INTO {qualified_table} VALUES (32)"))?;
    h.check(
        "S32 the same write runs in the new scope once the pin is gone",
        capture.results.first().is_some_and(|result| result.success),
        format!(
            "insert after unpinning: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    h.run("ROLLBACK")?;
    h.change_tab_scope(Some(&base_scope));
    let _ = h.editor.discard_pooled_session_for_close();
    let restored_scope = h.select_scalar(current_scope_sql)?;
    h.check(
        "S32 the tab is back in its own scope",
        restored_scope.trim().eq_ignore_ascii_case(&base_scope),
        format!("{current_scope_sql} = {restored_scope:?}"),
    );
    let _ = h.run(&if target.is_oracle() {
        format!("DROP USER {scratch_scope} CASCADE")
    } else {
        format!("DROP DATABASE IF EXISTS {scratch_scope}")
    });
    h.run("ROLLBACK")?;

    // ---- S40: a dirty session does not gate a scope change ------------------
    // The commit/rollback/discard decision belongs to tab close. A scope
    // change is applied to the tab's retained session IN PLACE (MySQL `USE`,
    // Oracle `ALTER SESSION SET CURRENT_SCHEMA`), so uncommitted work must
    // neither block it (the old ScopeChange preflight refused any preserved
    // state) nor be silently committed or discarded by it: the open
    // transaction simply continues in the new scope, still rollback-able.
    println!("  --- S40 a dirty session does not gate a scope change ---");
    h.editor.clear_tab_transaction_mode_override();
    let scratch_scope = "SQ_TM_SCOPE2";
    let _ = h.run(&if target.is_oracle() {
        format!("DROP USER {scratch_scope} CASCADE")
    } else {
        format!("DROP DATABASE IF EXISTS {scratch_scope}")
    });
    let capture = h.run(&if target.is_oracle() {
        format!("CREATE USER {scratch_scope} IDENTIFIED BY pw1")
    } else {
        format!("CREATE DATABASE {scratch_scope}")
    })?;
    if !capture.results.first().is_some_and(|result| result.success) {
        return Err(format!(
            "S40 could not create the scratch scope: {:?}",
            capture.results.first().map(|r| r.message.clone())
        ));
    }
    let capture = h.run(&format!("INSERT INTO {qualified_table} VALUES (4001)"))?;
    h.check(
        "S40 the uncommitted write before the scope change succeeds",
        capture.results.first().is_some_and(|result| result.success),
        format!(
            "insert result: {:?}",
            capture
                .results
                .first()
                .map(|r| (r.success, r.message.clone()))
        ),
    );
    let scope_outcome = h.change_tab_scope(Some(scratch_scope));
    h.check(
        "S40 the scope change is applied over the uncommitted work",
        scope_outcome.contains("Applied"),
        format!("scope change outcome: {scope_outcome}"),
    );
    let scope_now = h.select_scalar(current_scope_sql)?;
    h.check(
        "S40 the tab's session really moved while the transaction stayed open",
        scope_now.trim().eq_ignore_ascii_case(scratch_scope),
        format!("{current_scope_sql} = {scope_now:?}"),
    );
    let kept = h.select_scalar(&format!(
        "SELECT COUNT(*) FROM {qualified_table} WHERE V = 4001"
    ))?;
    h.check(
        "S40 the uncommitted work survived the scope change on the same session",
        kept.trim() == "1",
        format!("rows visible to the transaction = {kept:?}"),
    );
    h.run("ROLLBACK")?;
    let remaining = h.select_scalar(&format!(
        "SELECT COUNT(*) FROM {qualified_table} WHERE V = 4001"
    ))?;
    h.check(
        "S40 the work stayed rollback-able: nothing was committed by the scope change",
        remaining.trim() == "0",
        format!("rows after ROLLBACK = {remaining:?}"),
    );
    h.change_tab_scope(Some(&base_scope));
    let _ = h.editor.discard_pooled_session_for_close();
    let _ = h.run(&if target.is_oracle() {
        format!("DROP USER {scratch_scope} CASCADE")
    } else {
        format!("DROP DATABASE IF EXISTS {scratch_scope}")
    });
    h.run("ROLLBACK")?;

    // ---- S33 (MySQL family): XA transactions on a manual-commit tab ---------
    // `XA START` is the one transaction opener the server refuses over ANY
    // open transaction (XAER_OUTSIDE) instead of implicitly committing it —
    // and under autocommit=0 the app's own bookkeeping reads leave an
    // implicit transaction on the session. The pooled-session setup must
    // bring the session back to a boundary for it, the way it already does
    // for a one-shot SET TRANSACTION. And once inside an XA transaction, a
    // READ ONLY pin must still hold: the server refuses the write, so XA is
    // not an escape route around the pin.
    if !target.is_oracle() {
        println!("  --- S33 XA transactions respect the boundary and the pin ---");
        h.editor.clear_tab_transaction_mode_override();
        h.run("ROLLBACK")?;
        // Leave a read's implicit transaction on the session. MariaDB's dirty
        // probe (@@in_transaction) truthfully reports it as possibly-dirty, so
        // the app preserves it and XA START fails on the server — the user's
        // documented way out is resolving it, so the scenario does what a user
        // would and rolls back first; the boundary claim is still exercised by
        // whatever the acquisition's own bookkeeping opens after the ROLLBACK.
        // MySQL's probe answers "clean" for the implicit read-only
        // transaction, so the XA script must survive it WITHOUT a user
        // ROLLBACK — that is the exact sequence that used to die with
        // XAER_OUTSIDE.
        h.run("SELECT V FROM SQ_TM_T")?;
        if target == Target::MariaDb {
            h.run("ROLLBACK")?;
        }
        let capture = h.run(
            "XA START 'sq33';\nINSERT INTO SQ_TM_T VALUES (33);\nXA END 'sq33';\nXA COMMIT 'sq33' ONE PHASE;",
        )?;
        let failed: Vec<_> = capture
            .results
            .iter()
            .filter(|r| !r.success)
            .map(|r| (r.sql.clone(), r.message.clone()))
            .collect();
        h.check(
            "S33 the XA transaction runs over the session's residual read transaction",
            failed.is_empty(),
            format!("failed statements: {failed:?}"),
        );
        let landed = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 33")?;
        h.check(
            "S33 the XA commit really landed",
            landed.trim() == "1",
            format!("rows with V=33: {landed:?}"),
        );
        h.run("DELETE FROM SQ_TM_T WHERE V = 33")?;
        h.run("COMMIT")?;

        // A READ ONLY pin must hold inside an XA transaction too. The refusal
        // stops the batch (continue-on-error is off, the GUI default), which
        // leaves the session inside the still-ACTIVE XA transaction — plain
        // ROLLBACK cannot end one (XAER_RMFAIL), so the scenario must recover
        // the way a user would, with the XA verbs themselves. That recovery
        // path working IS part of the claim: the tab must not be bricked.
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        ));
        let capture = h.run(
            "XA START 'sq33ro';\nINSERT INTO SQ_TM_T VALUES (34);\nXA END 'sq33ro';\nXA ROLLBACK 'sq33ro';",
        )?;
        let insert = capture
            .results
            .iter()
            .find(|r| r.sql.contains("INSERT"))
            .ok_or("S33 XA read-only batch produced no INSERT result")?;
        h.check(
            "S33 the write inside the XA transaction is refused on the READ ONLY tab",
            !insert.success
                && read_only_errors.iter().any(|needle| {
                    insert
                        .message
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                }),
            format!(
                "insert inside XA: ({}, {:?})",
                insert.success, insert.message
            ),
        );
        h.check(
            "S33 the refusal stops the batch before the XA transaction ends",
            !capture
                .results
                .iter()
                .any(|r| r.sql.to_ascii_uppercase().contains("XA END")),
            format!(
                "batch results after the refusal: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.sql.clone(), r.success))
                    .collect::<Vec<_>>()
            ),
        );
        let escaped = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 34")?;
        h.check(
            "S33 no row escaped through the XA transaction",
            escaped.trim() == "0",
            format!("rows with V=34: {escaped:?}"),
        );
        let cleanup = h.run("XA END 'sq33ro';\nXA ROLLBACK 'sq33ro';")?;
        h.check(
            "S33 the XA verbs still recover the session after the refusal",
            cleanup.results.len() >= 2 && cleanup.results.iter().all(|r| r.success),
            format!(
                "cleanup results: {:?}",
                cleanup
                    .results
                    .iter()
                    .map(|r| (r.sql.clone(), r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        h.editor.clear_tab_transaction_mode_override();
        h.run("ROLLBACK")?;

        // The tab-close discard must also free a session that is stuck inside
        // an ACTIVE XA transaction: dropping the physical connection is the
        // one resolution that always works, because the server rolls an
        // ACTIVE XA transaction back on disconnect.
        h.run("XA START 'sq33x'")?;
        let _ = h.editor.discard_pooled_session_for_close();
        let capture = h.run("SELECT V FROM SQ_TM_T")?;
        h.check(
            "S33 discarding the session frees a tab stuck inside an XA transaction",
            capture.results.first().is_some_and(|result| result.success),
            format!(
                "first statement after the discard: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("ROLLBACK")?;
    }

    // ---- S34: the pinned isolation survives a DDL's implicit commit ---------
    // The DDL twin of S18b. Oracle expresses isolation as a TRANSACTION
    // property, and a DDL inside the user's batch commits implicitly — the
    // pin has to be re-applied to the transaction that follows, or every
    // statement after the DDL silently runs at the session default while the
    // toolbar still shows the pin. The MySQL family's SESSION characteristic
    // must survive the DDL on its own; asserting it keeps the claim honest on
    // all four targets.
    println!("  --- S34 the pinned isolation survives a DDL's implicit commit ---");
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    {
        let mut other = attach_tab(connect_target(target)?);
        let pinned_isolation = if target.is_oracle() {
            TransactionIsolation::Serializable
        } else {
            // REPEATABLE READ, not SERIALIZABLE: InnoDB's SERIALIZABLE turns
            // plain reads into locking reads, which would block the other
            // session's UPDATE instead of testing the snapshot.
            TransactionIsolation::RepeatableRead
        };
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            pinned_isolation,
            TransactionAccessMode::ReadWrite,
        ));
        let sleep_statement = if target.is_oracle() {
            "BEGIN DBMS_SESSION.SLEEP(6); END;\n/"
        } else {
            "DO SLEEP(6);"
        };
        let ddl = if target.is_oracle() {
            "CREATE TABLE SQ_TM_DDL (V NUMBER);\n"
        } else {
            "CREATE TABLE SQ_TM_DDL (V INT);\n"
        };
        let (reads, failed) = bracketed_reads_in_one_batch(h, &mut other, ddl, sleep_statement)?;
        h.check(
            "S34 every statement of the DDL batch ran",
            failed.is_empty(),
            format!("failed statements: {failed:?}"),
        );
        let after_ddl_pair = last_pair(&reads);
        h.check(
            "S34 both reads after the DDL's implicit commit see one transaction snapshot",
            after_ddl_pair.is_some_and(|(first, second)| first == second),
            format!(
                "reads in the batch: {reads:?} (the last two bracket the other \
                 session's commit; different values mean the pinned isolation \
                 was dropped by the DDL's implicit commit)"
            ),
        );
        h.run("ROLLBACK")?;
        // Control: the other session really did commit, so the check above
        // cannot pass by nothing having happened.
        h.editor.clear_tab_transaction_mode_override();
        let now = h.select_scalar("SELECT V FROM SQ_TM_ISO")?;
        h.check(
            "S34 the other session's commit is visible to a new transaction",
            now.trim()
                .parse::<i64>()
                .ok()
                .zip(after_ddl_pair)
                .is_some_and(|(now, (first, _))| now > first),
            format!("SQ_TM_ISO after the batch = {now:?}, in-batch reads {reads:?}"),
        );
        let _ = other.run("ROLLBACK");
        let _ = other.editor.discard_pooled_session_for_close();
    }
    h.editor.clear_tab_transaction_mode_override();
    h.run("ROLLBACK")?;
    // The sleeping statement leaves conservative session residue (a PL/SQL
    // block on Oracle, an unclassified command on the MySQL family); discard
    // the session the way the tab's close path does so it cannot block the
    // scenario after this one.
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S35 (MySQL family): assignment spellings of the READ WRITE escape --
    // The one-shot of S30 exists in two more spellings the server honours over
    // the READ ONLY session characteristic (both raw-verified to land a
    // write): `SET @@transaction_read_only = 0` — bare @@ is next-transaction
    // scope, and the pending value is INVISIBLE to a readback of the same
    // variable, so the settings fast path cannot catch it — and MariaDB's
    // statement-scoped `SET STATEMENT transaction_read_only=0 FOR <write>`.
    // The client gate must refuse them like the word forms, and pin WHICH
    // defense refused: the client message, not a server error.
    if !target.is_oracle() {
        println!("  --- S35 assignment spellings cannot escape a READ ONLY tab ---");
        h.toolbar_transaction_mode(TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        ));
        let capture =
            h.run("SET @@transaction_read_only = 0;\nINSERT INTO SQ_TM_T VALUES (35);")?;
        let first = capture.results.first().cloned();
        h.check(
            "S35 the @@ one-shot escape is refused by the client gate",
            first.as_ref().is_some_and(|result| {
                !result.success
                    && result
                        .message
                        .to_ascii_lowercase()
                        .contains("read only mode blocks")
            }),
            format!(
                "one-shot assignment escape: {:?}",
                first.map(|r| (r.success, r.message))
            ),
        );
        h.check(
            "S35 the refusal stops the batch before the write",
            !capture
                .results
                .iter()
                .any(|result| result.sql.to_ascii_uppercase().contains("INSERT")),
            format!(
                "batch results: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.sql.clone(), r.success))
                    .collect::<Vec<_>>()
            ),
        );
        if target == Target::MariaDb {
            let capture =
                h.run("SET STATEMENT transaction_read_only=0 FOR INSERT INTO SQ_TM_T VALUES (35)")?;
            let first = capture.results.first().cloned();
            h.check(
                "S35 the SET STATEMENT escape is refused by the client gate",
                first.as_ref().is_some_and(|result| {
                    !result.success
                        && result
                            .message
                            .to_ascii_lowercase()
                            .contains("read only mode blocks")
                }),
                format!(
                    "SET STATEMENT escape: {:?}",
                    first.map(|r| (r.success, r.message))
                ),
            );
        }
        h.editor.clear_tab_transaction_mode_override();
        let _ = h.editor.discard_pooled_session_for_close();
        let leaked = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 35")?;
        h.check(
            "S35 no row landed through an assignment-spelling escape",
            leaked.trim() == "0",
            format!("COUNT(*) WHERE V = 35 = {leaked}"),
        );
        h.run("ROLLBACK")?;
    }

    // ---- S36 (MySQL family): the @@ one-shot gets its transaction boundary --
    // The word form `SET TRANSACTION ...` must be the first statement of its
    // transaction and the pooled-session setup prepares a boundary for it; the
    // assignment spelling `SET @@transaction_isolation = ...` hits the same
    // ER 1568 over the implicit transaction the app's own bookkeeping reads
    // leave behind under autocommit=0 — failing for a transaction the user
    // never opened. It must run like the word form, and stay a one-shot: no
    // tab pin, no UI event.
    if !target.is_oracle() {
        println!("  --- S36 the @@ one-shot works like SET TRANSACTION ---");
        h.editor.clear_tab_transaction_mode_override();
        let _ = h.editor.discard_pooled_session_for_close();
        // Leave a read on the session, like S33: MariaDB's truthful probe
        // preserves the read transaction (the user's documented way out is
        // ROLLBACK), while MySQL's probe answers "clean" for it, so the
        // one-shot must survive the session's residual transaction WITHOUT a
        // user ROLLBACK — the exact sequence that used to die with ER 1568.
        h.run("SELECT V FROM SQ_TM_T")?;
        if target == Target::MariaDb {
            h.run("ROLLBACK")?;
        }
        let capture = h.run("SET @@transaction_isolation = 'SERIALIZABLE'")?;
        h.check(
            "S36 the isolation one-shot runs over the session's residual transaction",
            capture.results.first().is_some_and(|result| result.success),
            format!(
                "one-shot assignment: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.check(
            "S36 the one-shot pins nothing on the tab",
            h.editor.tab_transaction_mode_override_value().is_none()
                && capture.mode_changes.is_empty(),
            format!(
                "override: {:?}, mode change events: {:?}",
                h.editor.tab_transaction_mode_override_value(),
                capture.mode_changes
            ),
        );
        // Consume the pending one-shot the way S4 does (single statements on
        // purpose: a script never classifies as the consumer).
        h.run("START TRANSACTION")?;
        h.run("ROLLBACK")?;
        // The read-only direction proves the one-shot really reached the
        // server: on an UNPINNED tab the client gate stays out of the way,
        // its consumer is refused by the SERVER, and the transaction after
        // that one runs read-write again — one-shot semantics end to end.
        let capture = h.run("SET @@transaction_read_only = 1")?;
        h.check(
            "S36 the read-only one-shot runs on an unpinned tab",
            capture.results.first().is_some_and(|result| result.success),
            format!(
                "read-only one-shot: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        let refused = h.run("INSERT INTO SQ_TM_T VALUES (36)")?;
        h.check(
            "S36 the server refuses the one-shot's consumer write",
            refused.results.first().is_some_and(|result| {
                !result.success
                    && result
                        .message
                        .to_ascii_lowercase()
                        .contains("read only transaction")
            }),
            format!(
                "consumer write: {:?}",
                refused
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        // The refused consumer leaves the session inside the one-shot's
        // read-only transaction with a failed DML on it — conservative
        // residue that requires resolution, so ANY typed statement would pop
        // the resolution dialog (and hang the harness). Resolve it the way
        // the tab-close path does, like S18/S33: discard the session.
        let _ = h.editor.discard_pooled_session_for_close();
        let after = h.run("INSERT INTO SQ_TM_T VALUES (36)")?;
        h.check(
            "S36 the tab writes again once the one-shot's session is resolved",
            after.results.first().is_some_and(|result| result.success),
            format!(
                "write after the one-shot: {:?}",
                after
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("ROLLBACK")?;
    }

    // ---- S37 (Oracle): adoption over a READ ONLY pin, both directions ------
    // Oracle's only query-driven session-persistent mode change is ALTER
    // SESSION SET ISOLATION_LEVEL. Over a READ ONLY pin the merge lands on an
    // (isolation, READ ONLY) pair, and those pairs split: Serializable IS
    // what a read-only Oracle transaction provides (one consistent snapshot),
    // so that adoption must go through; Read committed cannot exist inside a
    // read-only transaction, so that adoption must be refused rather than
    // pinning a pair the toolbar refuses to select — pre-fix it killed the
    // OCI batch at its next boundary re-application and left the session on
    // the abandoned level with the toolbar reading Default. The refusal keeps
    // the conservative session residue instead, which the discard resolves.
    if target.is_oracle() {
        println!("  --- S37 an adoption the database cannot express is refused ---");
        let read_only_pin = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );
        h.editor.set_tab_transaction_mode(read_only_pin);
        let capture = h.run("ALTER SESSION SET ISOLATION_LEVEL = READ COMMITTED")?;
        h.check(
            "S37 the ALTER SESSION statement itself succeeds",
            capture.results.first().is_some_and(|r| r.success),
            format!(
                "result: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.check(
            "S37 the tab override keeps the READ ONLY pin, not the merged pair",
            h.editor.tab_transaction_mode_override_value() == Some(read_only_pin),
            format!(
                "override: {:?}",
                h.editor.tab_transaction_mode_override_value()
            ),
        );
        h.check(
            "S37 no TransactionModeChanged event fires for the refused adoption",
            capture.mode_changes.is_empty(),
            format!("mode change events: {:?}", capture.mode_changes),
        );
        let residue = h
            .editor
            .pooled_session_activity_snapshot()
            .map(|snapshot| snapshot.retained_state());
        h.check(
            "S37 the conservative session residue is kept",
            residue.is_some_and(|state| {
                state.may_have_transaction_mode_override() || state.requires_transaction_decision()
            }),
            format!("retained state after the refused adoption = {residue:?}"),
        );
        // The residue requires resolution, so a typed statement would pop the
        // resolution dialog (and hang the harness). Resolve it the way the
        // tab-close path does: discard the session, which also takes the
        // session-persistent isolation with it.
        let _ = h.editor.discard_pooled_session_for_close();
        let v = h.select_v()?;
        h.check(
            "S37 the pinned tab still reads after the discard",
            v >= 1,
            format!("V = {v}"),
        );
        let refused = h.run("INSERT INTO SQ_TM_T VALUES (37)")?;
        h.check(
            "S37 the READ ONLY pin still refuses the write",
            refused.results.first().is_some_and(|result| {
                !result.success
                    && read_only_errors.iter().any(|needle| {
                        result
                            .message
                            .to_ascii_lowercase()
                            .contains(&needle.to_ascii_lowercase())
                    })
            }),
            format!(
                "insert result: {:?}",
                refused
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("ROLLBACK")?;
        let _ = h.editor.discard_pooled_session_for_close();

        // The expressible direction: SERIALIZABLE over the same pin adopts,
        // the pair reads, and the pin still refuses the write.
        println!("  --- S37b a Serializable adoption over the READ ONLY pin goes through ---");
        h.editor.set_tab_transaction_mode(read_only_pin);
        let serializable_read_only = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        let capture = h.run("ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE")?;
        h.check(
            "S37b the ALTER SESSION statement succeeds",
            capture.results.first().is_some_and(|r| r.success),
            format!(
                "result: {:?}",
                capture
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.check(
            "S37b the tab adopts Serializable + Read only",
            h.editor.tab_transaction_mode_override_value() == Some(serializable_read_only),
            format!(
                "override: {:?}",
                h.editor.tab_transaction_mode_override_value()
            ),
        );
        h.check(
            "S37b the adoption notifies the UI",
            capture.mode_changes.last() == Some(&serializable_read_only),
            format!("mode change events: {:?}", capture.mode_changes),
        );
        let v = h.select_v()?;
        h.check(
            "S37b the adopted pair still reads",
            v >= 1,
            format!("V = {v}"),
        );
        let refused = h.run("INSERT INTO SQ_TM_T VALUES (37)")?;
        h.check(
            "S37b the adopted pair still refuses the write",
            refused.results.first().is_some_and(|result| {
                !result.success
                    && read_only_errors.iter().any(|needle| {
                        result
                            .message
                            .to_ascii_lowercase()
                            .contains(&needle.to_ascii_lowercase())
                    })
            }),
            format!(
                "insert result: {:?}",
                refused
                    .results
                    .first()
                    .map(|r| (r.success, r.message.clone()))
            ),
        );
        h.run("ROLLBACK")?;
        // The adopted ALTER SESSION is session-persistent; the harness clear
        // below skips the GUI's Default-selection reset, so discard the
        // session to shed the abandoned level.
        let _ = h.editor.discard_pooled_session_for_close();
        h.editor.clear_tab_transaction_mode_override();
    }

    // ---- S38: a SAVEPOINT survives across executions under a pinned mode ---
    // Savepoints only exist inside one continuous transaction on one session,
    // so this is the sharpest cross-execution probe the feature has: if the
    // app re-applies the pinned mode mid-transaction (Oracle: ORA-01453), or
    // the MySQL family's per-statement session setup slips a ROLLBACK between
    // the user's statements (the S18 BUG B shape), the second execution's
    // ROLLBACK TO SAVEPOINT fails ("savepoint does not exist") instead of
    // silently running under a broken transaction.
    println!("  --- S38 a savepoint survives across executions under a pin ---");
    h.editor.set_tab_transaction_mode(TransactionMode::new(
        TransactionIsolation::Serializable,
        TransactionAccessMode::ReadWrite,
    ));
    let first = h.run(
        "INSERT INTO SQ_TM_T VALUES (3801);\n\
         SAVEPOINT SQ_TM_SP;\n\
         INSERT INTO SQ_TM_T VALUES (3802);\n",
    )?;
    h.check(
        "S38 the batch that creates the savepoint runs whole",
        first.results.len() >= 3 && first.results.iter().all(|r| r.success),
        format!(
            "results: {:?}",
            first
                .results
                .iter()
                .map(|r| (r.success, r.message.clone()))
                .collect::<Vec<_>>()
        ),
    );
    let second = h.run(
        "ROLLBACK TO SAVEPOINT SQ_TM_SP;\n\
         INSERT INTO SQ_TM_T VALUES (3803);\n",
    )?;
    h.check(
        "S38 the next execution still sees the savepoint",
        second.results.len() >= 2 && second.results.iter().all(|r| r.success),
        format!(
            "results: {:?}",
            second
                .results
                .iter()
                .map(|r| (r.success, r.message.clone()))
                .collect::<Vec<_>>()
        ),
    );
    let kept = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V IN (3801, 3803)")?;
    let discarded = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 3802")?;
    h.check(
        "S38 the partial rollback kept exactly the rows outside the savepoint",
        kept.trim() == "2" && discarded.trim() == "0",
        format!("rows kept (3801,3803) = {kept:?}, rolled back (3802) = {discarded:?}"),
    );
    h.run("ROLLBACK")?;
    let remaining =
        h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V IN (3801, 3802, 3803)")?;
    h.check(
        "S38 everything was one open transaction: the rollback takes it all back",
        remaining.trim() == "0",
        format!("rows left after ROLLBACK = {remaining:?}"),
    );
    h.editor.clear_tab_transaction_mode_override();
    // SAVEPOINT tracking leaves conservative session-bound residue on the
    // MySQL family; discard the session the way the tab-close path does so it
    // cannot block the next scenario.
    let _ = h.editor.discard_pooled_session_for_close();

    // ---- S39 (MySQL family): COMMIT AND CHAIN under the tab's pin ----------
    // AND CHAIN is the one transaction OPENER the client gate deliberately
    // lets through on every tab: the chained transaction inherits the
    // isolation level AND access mode of the one it commits (raw-verified on
    // both servers). Two halves: the chained transaction must be tracked as
    // open user work (rollback-able, close prompt truthful), and it must NOT
    // be an escape route around a READ ONLY pin.
    if !target.is_oracle() {
        println!("  --- S39 COMMIT AND CHAIN keeps the chain and the pin ---");
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::RepeatableRead,
            TransactionAccessMode::ReadWrite,
        ));
        let capture = h.run(
            "START TRANSACTION;\n\
             INSERT INTO SQ_TM_T VALUES (3901);\n\
             COMMIT AND CHAIN;\n\
             INSERT INTO SQ_TM_T VALUES (3902);\n",
        )?;
        h.check(
            "S39 the chained batch runs whole",
            capture.results.len() >= 4 && capture.results.iter().all(|r| r.success),
            format!(
                "results: {:?}",
                capture
                    .results
                    .iter()
                    .map(|r| (r.success, r.message.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        let chained_state = h
            .editor
            .pooled_session_activity_snapshot()
            .map(|snapshot| snapshot.retained_state());
        h.check(
            "S39 the chained transaction is tracked as open user work",
            chained_state.is_some_and(|state| state.may_have_uncommitted_work()),
            format!("retained state after COMMIT AND CHAIN = {chained_state:?}"),
        );
        h.run("ROLLBACK")?;
        let committed = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 3901")?;
        let chained = h.select_scalar("SELECT COUNT(*) FROM SQ_TM_T WHERE V = 3902")?;
        h.check(
            "S39 AND CHAIN committed the first transaction and the rollback took the second",
            committed.trim() == "1" && chained.trim() == "0",
            format!("committed (3901) = {committed:?}, chained (3902) = {chained:?}"),
        );
        h.run("DELETE FROM SQ_TM_T WHERE V = 3901")?;
        h.run("COMMIT")?;

        // The READ ONLY half: the chained transaction inherits the pinned
        // access mode, so the write after AND CHAIN is refused by the server
        // (ER 1792) — chaining is not an escape.
        h.editor.set_tab_transaction_mode(TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        ));
        let capture = h.run(
            "SELECT V FROM SQ_TM_T;\n\
             COMMIT AND CHAIN;\n\
             INSERT INTO SQ_TM_T VALUES (3903);\n",
        )?;
        let insert_result = capture.results.last().cloned();
        h.check(
            "S39 the write after COMMIT AND CHAIN is still refused on a READ ONLY tab",
            capture.results.len() >= 3
                && insert_result.as_ref().is_some_and(|result| {
                    !result.success
                        && read_only_errors.iter().any(|needle| {
                            result
                                .message
                                .to_ascii_lowercase()
                                .contains(&needle.to_ascii_lowercase())
                        })
                }),
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
        let _ = h.editor.discard_pooled_session_for_close();
    }

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

/// The database (MySQL family) or schema (Oracle) a tab on this target runs
/// in with no scope selected — what a scope change moves away from, and what
/// qualifies a table name while the tab is somewhere else.
fn base_scope(target: Target) -> String {
    let info = target.connection_info();
    if target.is_oracle() {
        info.username.to_uppercase()
    } else {
        info.service_name
    }
}

/// Every isolation level the toolbar offers for this backend, minus `Default`
/// — which is not a level of its own but "whatever the connection default is",
/// and is covered by S11/S16.
fn offered_isolations(target: Target) -> Vec<TransactionIsolation> {
    let db_type = match target {
        Target::OracleOci | Target::OracleThin => DatabaseType::Oracle,
        Target::MySql => DatabaseType::MySQL,
        Target::MariaDb => DatabaseType::MariaDB,
    };
    db_type
        .supported_transaction_isolations()
        .iter()
        .copied()
        .filter(|isolation| *isolation != TransactionIsolation::Default)
        .collect()
}

/// The MySQL family reports a level as `READ-COMMITTED`; the app's own label
/// is `Read committed`. Compare them the way the app's parser does — on
/// separators and case.
fn isolation_value_matches(session_value: &str, expected: TransactionIsolation) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .replace(['-', '_'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase()
    };
    normalize(session_value) == normalize(expected.label())
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
    // The scenarios open several tabs on one connection, and each tab holds a
    // pooled session of its own; the shipped default pool is smaller than this
    // harness needs, and running out of it fails a scenario for a reason that
    // has nothing to do with what it verifies.
    connection.set_connection_pool_size(space_query::utils::config::MAX_CONNECTION_POOL_SIZE);
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
    // This harness drives the real editor through 20+ execution scenarios per
    // backend, so it observes far more of the app's lock graph than a targeted
    // run does. Fail on any inversion it saw.
    all_failures.extend(space_query::db::lock_order::report_observed_lock_order(
        "transaction mode harness",
    ));

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
