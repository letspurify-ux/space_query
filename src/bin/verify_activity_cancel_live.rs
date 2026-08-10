#![allow(clippy::cargo, clippy::pedantic)]

// Live verification that the DB activity registry actually ends real work.
//
// Unit tests cover the registry's bookkeeping with a fake canceler. This binary
// proves the part that only a real server can answer: that the canceler built
// at session acquire reaches a statement the driver is genuinely blocked in,
// on every backend, for both tiers.
//
// Scenarios per target:
//   A1  a long-running statement on a pooled session is visible in the activity
//       registry and reports itself cancelable.
//   A2  cancel_db_activity breaks that statement: the worker returns with an
//       error (not a result), and it returns promptly rather than running the
//       statement to completion.
//   A3  the status entry is retired the moment the cancel is dispatched, so
//       nothing keeps reading as in progress while the worker unwinds.
//   A4  force tier: with a zero cancel timeout the watchdog escalates and the
//       call still ends.
//   A5  session teardown: after the pool context epoch is bumped (what every
//       disconnect does) the stale sweep retires the activity and breaks the
//       statement, with no cancel button involved.
//   A6  a session returned to the pool detaches its canceler, so a later cancel
//       cannot break an unrelated statement on the recycled session.
//   A7  a REAL editor query — driven through SqlEditorWidget exactly as the GUI
//       does — is cancelable through the activity registry for its whole run,
//       not just while its session is being acquired. This is the scenario the
//       unit tests cannot reach: it proves the operation's status entry and the
//       thing the cancel button ends are the same object.
//   A8  the same, on a RETAINED session: the tab is left holding an open
//       transaction, so the next query reuses its session instead of acquiring
//       one. That is the common case in real use, and it skips `acquire`
//       entirely — the path a pool-acquire-only invariant misses.
//   A9  a cancel that comes from the registry (a disconnect, or the stale
//       sweep) is reported to the user as a cancel, not as a driver error.
//
// Usage: verify_activity_cancel_live <thin|oci|mysql|mariadb|all>

use fltk::{app, input::IntInput};
use space_query::db::{
    active_db_activity_snapshots, cancel_db_activity, clear_tracked_db_activity,
    sweep_stale_db_activities, track_pool_db_activity, ConnectionInfo, DatabaseConnection,
    DatabaseType, DbPoolSession, OracleDriverMode,
};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
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

    /// A statement that blocks server-side for far longer than any assertion
    /// window, so a test that "passes" cannot be a statement that simply
    /// finished on its own.
    /// Leaves the tab holding an open transaction, so its session is retained
    /// and the next statement takes the reuse path.
    fn retain_session_sql(self) -> &'static str {
        if self.is_oracle() {
            "INSERT INTO SQ_CANCEL_T VALUES (1)"
        } else {
            "START TRANSACTION; INSERT INTO SQ_CANCEL_T VALUES (1)"
        }
    }

    fn setup_sql(self) -> Vec<&'static str> {
        if self.is_oracle() {
            vec!["DROP TABLE SQ_CANCEL_T", "CREATE TABLE SQ_CANCEL_T (V NUMBER)"]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_CANCEL_T",
                "CREATE TABLE SQ_CANCEL_T (V INT)",
            ]
        }
    }

    fn slow_sql(self) -> &'static str {
        if self.is_oracle() {
            // NOT a DBMS_SESSION.SLEEP: that is uninterruptible server-side, so
            // it would measure Oracle's sleep rather than this app's cancel. A
            // heavy join is what real user queries look like and is what
            // OCIBreak / the thin break are meant to stop.
            "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c WHERE a.object_id > 0"
        } else {
            // NOT a SLEEP(): MySQL's SLEEP returns success when killed, so it
            // could not tell a landed cancel from a completed statement. A
            // heavy join is interrupted with a real error, like a user query.
            "SELECT COUNT(*) FROM information_schema.COLUMNS a,              information_schema.COLUMNS b, information_schema.COLUMNS c"
        }
    }
}

/// The configured cancel timeout the probes run under. Tier 1 gets this long
/// to land before the watchdog forces the session closed.
const PROBE_CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

/// The contract under test: a cancel ends the work by the cancel timeout, plus
/// room for the watchdog poll and a server round trip. Which tier did it is
/// reported, not asserted — Oracle thin's graceful break needs OOB, which this
/// test DB does not offer, so tier 2 legitimately does the work there.
fn cancel_deadline() -> Duration {
    PROBE_CANCEL_TIMEOUT + Duration::from_secs(20)
}

/// The probe statements all run for minutes when left alone, and each scenario
/// first checks the statement is still running before cancelling. So stopping
/// under this bound cannot be the statement completing by itself — it is the
/// cancel landing.
const RAN_TO_COMPLETION_FLOOR: Duration = Duration::from_secs(15);

/// The editor's cancel path settles the operation through the result tab's
/// status and emits no statement result, so this stands in for "cancelled".
const NO_STATEMENT_RESULT: &str = "(no statement result)";

struct Harness {
    target: Target,
    connection: Arc<Mutex<DatabaseConnection>>,
}

impl Harness {
    fn connect(target: Target) -> Result<Self, String> {
        let mut connection = DatabaseConnection::new();
        connection.connect(target.connection_info())?;
        // Two slots so a test can hold one session and still acquire another.
        connection.set_connection_pool_size(4);
        Ok(Self {
            target,
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn pool_context(&self) -> Result<space_query::db::DbPoolSessionContext, String> {
        space_query::db::pool_session_context_for_shared_connection(&self.connection, None)
    }
}

/// Runs the target's slow statement on a pooled session under `activity`, and
/// reports how it ended.
struct SlowStatement {
    finished: Arc<AtomicBool>,
    outcome: Arc<Mutex<Option<String>>>,
    started_at: Instant,
    handle: Option<thread::JoinHandle<()>>,
}

impl SlowStatement {
    fn spawn(
        harness: &Harness,
        activity: space_query::db::DbActivityGuard,
    ) -> Result<Self, String> {
        let context = harness.pool_context()?;
        let target = harness.target;
        let finished = Arc::new(AtomicBool::new(false));
        let acquired = Arc::new(AtomicBool::new(false));
        let outcome: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let finished_in_worker = finished.clone();
        let outcome_in_worker = outcome.clone();
        let acquired_in_worker = acquired.clone();
        let handle = thread::spawn(move || {
            let outcome = (|| -> Result<(), String> {
                let (session, _cancel_registration) =
                    context.acquire_session_for_current_scope(&activity)?;
                acquired_in_worker.store(true, Ordering::Release);
                run_slow_statement(session, target.slow_sql())
            })();
            *outcome_in_worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(match &outcome {
                Ok(()) => "completed without error".to_string(),
                Err(message) => message.clone(),
            });
            finished_in_worker.store(true, Ordering::Release);
        });

        // Do not start asserting until the statement is genuinely in flight.
        if !wait_until(Duration::from_secs(30), || {
            acquired.load(Ordering::Acquire)
        }) {
            return Err("slow statement never acquired its session".into());
        }
        thread::sleep(Duration::from_millis(400));

        Ok(Self {
            finished,
            outcome,
            started_at: Instant::now(),
            handle: Some(handle),
        })
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn wait_for_finish(&self, within: Duration) -> bool {
        wait_until(within, || self.is_finished())
    }

    fn outcome(&self) -> String {
        self.outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| "still running".to_string())
    }

    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SlowStatement {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_slow_statement(session: DbPoolSession, sql: &str) -> Result<(), String> {
    match session {
        DbPoolSession::Oracle(conn) => conn
            .query_row(sql, &[])
            .map(|_| ())
            .map_err(|err| err.to_string()),
        DbPoolSession::OracleThin(conn) => {
            let mut conn = *conn;
            conn.query_drop(sql).map_err(|err| err.to_string())
        }
        DbPoolSession::MySQL { mut conn, .. } => {
            use mysql::prelude::Queryable;
            conn.as_mut()
                .query_drop(sql)
                .map_err(|err| err.to_string())
        }
    }
}

fn wait_until(within: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    ready()
}

/// Which tier ended the call, judged by whether it stopped before the force
/// deadline. Reported so a backend silently losing tier 1 is visible.
fn tier_label(elapsed: Duration) -> &'static str {
    if elapsed < PROBE_CANCEL_TIMEOUT {
        "tier 1 (graceful break)"
    } else {
        "tier 2 (force close)"
    }
}

fn snapshot_for(id: u64) -> Option<space_query::db::DbActivitySnapshot> {
    active_db_activity_snapshots()
        .into_iter()
        .find(|activity| activity.id == id)
}

fn verify(target: Target) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let harness = Harness::connect(target)?;

    // ---- A1..A3: the cancel button path -----------------------------------
    clear_tracked_db_activity();
    {
        let activity = track_pool_db_activity("Live cancel probe", target.connection_info().db_type);
        let activity_id = activity.id();
        let statement = SlowStatement::spawn(&harness, activity.clone())?;

        match snapshot_for(activity_id) {
            None => failures.push("A1: running statement is not in the activity registry".into()),
            Some(snapshot) if !snapshot.cancelable => {
                failures.push("A1: running statement is not reported as cancelable".into())
            }
            Some(_) => {}
        }
        if statement.is_finished() {
            failures.push("A1: the probe statement finished on its own; it is not slow enough".into());
        }

        let cancelled = cancel_db_activity(activity_id, PROBE_CANCEL_TIMEOUT);
        if !cancelled {
            failures.push("A2: cancel_db_activity did not find the running activity".into());
        }

        if snapshot_for(activity_id).is_some() {
            failures.push("A3: the status entry survived the cancel".into());
        }
        if !activity.is_finished() {
            failures.push("A3: the worker cannot see that it was cancelled".into());
        }

        if !statement.wait_for_finish(cancel_deadline()) {
            failures.push(format!(
                "A2: statement still running {:?} after cancel; the cancel timeout did not end it",
                statement.elapsed()
            ));
        } else {
            if statement.elapsed() >= RAN_TO_COMPLETION_FLOOR {
                failures.push(format!(
                    "A2: statement took {:?} to stop, which is indistinguishable from running to completion",
                    statement.elapsed()
                ));
            }
            println!(
                "   A2 ended after {:?} via {} [driver reported: {}]",
                statement.elapsed(),
                tier_label(statement.elapsed()),
                statement.outcome()
            );
        }
        statement.join();
    }

    // ---- A4: force tier ----------------------------------------------------
    clear_tracked_db_activity();
    {
        let activity = track_pool_db_activity("Live force probe", target.connection_info().db_type);
        let activity_id = activity.id();
        let statement = SlowStatement::spawn(&harness, activity.clone())?;

        // Zero timeout means the watchdog escalates immediately.
        cancel_db_activity(activity_id, Duration::ZERO);

        if !statement.wait_for_finish(cancel_deadline()) {
            failures.push(format!(
                "A4: statement survived the force tier for {:?}",
                statement.elapsed()
            ));
        } else {
            if statement.elapsed() >= RAN_TO_COMPLETION_FLOOR {
                failures.push(format!(
                    "A4: statement took {:?} to stop under the force tier",
                    statement.elapsed()
                ));
            }
            println!(
                "   A4 force tier ended it after {:?} [driver reported: {}]",
                statement.elapsed(),
                statement.outcome()
            );
        }
        statement.join();
    }

    // ---- A5: session teardown, no cancel button involved --------------------
    clear_tracked_db_activity();
    {
        let activity = track_pool_db_activity("Live sweep probe", target.connection_info().db_type);
        let activity_id = activity.id();
        let statement = SlowStatement::spawn(&harness, activity.clone())?;

        if sweep_stale_db_activities(PROBE_CANCEL_TIMEOUT) != 0 {
            failures.push("A5: sweep retired a live activity".into());
        }

        // What every disconnect does: end the connection's sessions.
        {
            let mut guard = space_query::db::lock_connection(&harness.connection);
            guard.disconnect();
        }

        let swept = wait_until(Duration::from_secs(10), || {
            sweep_stale_db_activities(PROBE_CANCEL_TIMEOUT) > 0
                || snapshot_for(activity_id).is_none()
        });
        if !swept {
            failures.push("A5: sweep did not retire the activity after the session ended".into());
        }
        if snapshot_for(activity_id).is_some() {
            failures.push("A5: a finished session left work showing as in progress".into());
        }
        if !statement.wait_for_finish(cancel_deadline()) {
            failures.push(format!(
                "A5: statement kept running {:?} after its session ended",
                statement.elapsed()
            ));
        } else {
            if statement.elapsed() >= RAN_TO_COMPLETION_FLOOR {
                failures.push(format!(
                    "A5: statement took {:?} to stop after its session ended",
                    statement.elapsed()
                ));
            }
            println!(
                "   A5 ended after {:?} via {} [driver reported: {}]",
                statement.elapsed(),
                tier_label(statement.elapsed()),
                statement.outcome()
            );
        }
        statement.join();
    }

    // ---- A6: a returned session must not be broken by a later cancel --------
    clear_tracked_db_activity();
    {
        let harness = Harness::connect(target)?;
        let context = harness.pool_context()?;
        let first = track_pool_db_activity("Live detach probe", target.connection_info().db_type);
        {
            let (_session, _cancel_registration) =
                context.acquire_session_for_current_scope(&first)?;
        }
        // The session went back to the pool; the activity must no longer claim
        // it can cancel anything.
        match snapshot_for(first.id()) {
            Some(snapshot) if snapshot.cancelable => {
                failures.push("A6: a released session is still attached to its activity".into())
            }
            _ => {}
        }

        let second = track_pool_db_activity("Live victim probe", target.connection_info().db_type);
        let victim = SlowStatement::spawn(&harness, second.clone())?;
        cancel_db_activity(first.id(), PROBE_CANCEL_TIMEOUT);
        thread::sleep(Duration::from_secs(2));
        if victim.is_finished() {
            failures.push(
                "A6: cancelling a finished activity broke an unrelated statement on the recycled session"
                    .into(),
            );
        }
        cancel_db_activity(second.id(), PROBE_CANCEL_TIMEOUT);
        victim.wait_for_finish(cancel_deadline());
        victim.join();
    }

    // ---- A7/A8/A9: real editor queries, cancelled through the registry -----
    for retained in [false, true] {
        clear_tracked_db_activity();
        let label = if retained { "A8" } else { "A7" };
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);

        if retained {
            for sql in target.setup_sql() {
                // DROP of a missing table is expected to fail on the first run.
                let _ = editor.run(sql, Duration::from_secs(30));
            }
            if !editor.run(target.retain_session_sql(), Duration::from_secs(30)) {
                failures.push(format!("{label}: could not open a transaction to retain the session"));
            }
            if editor.retained_session_state().is_none() {
                failures.push(format!(
                    "{label}: the tab did not retain its session, so this is not the reuse path"
                ));
            }
        }

        editor.start(target.slow_sql());
        let cancelable = editor.pump_until(Duration::from_secs(30), || {
            active_db_activity_snapshots()
                .into_iter()
                .find(|activity| activity.cancelable)
        });

        match cancelable {
            None => failures.push(format!(
                "{label}: a running editor query never became cancelable through the registry"
            )),
            Some(target_activity) => {
                if editor.is_done() {
                    failures.push(format!("{label}: the editor query finished before it could be cancelled"));
                }
                let started = Instant::now();
                if !cancel_db_activity(target_activity.id, PROBE_CANCEL_TIMEOUT) {
                    failures.push(format!("{label}: cancel_db_activity did not find the editor query"));
                }
                if !editor.wait_done(cancel_deadline()) {
                    failures.push(format!(
                        "{label}: editor query still running {:?} after a registry cancel",
                        started.elapsed()
                    ));
                } else {
                    if started.elapsed() >= RAN_TO_COMPLETION_FLOOR {
                        failures.push(format!(
                            "{label}: editor query took {:?} to stop",
                            started.elapsed()
                        ));
                    }
                    println!(
                        "   {label} {} query ended after {:?} via {}",
                        if retained { "retained-session" } else { "fresh-session" },
                        started.elapsed(),
                        tier_label(started.elapsed())
                    );
                }
                if snapshot_for(target_activity.id).is_some() {
                    failures.push(format!("{label}: the query's status entry survived the cancel"));
                }

                // A9: the user must be told this was a cancel, not a failure.
                // A registry cancel must land on the same path as the cancel
                // button. That path reports through the result tab's status and
                // deliberately emits no statement result, so the check is that
                // no driver ERROR reached the user: before this was wired up the
                // query surfaced "Error: ORA-01013 ..." as a plain failure.
                // Either the cancel path suppressed the statement result (Oracle)
                // or it reported the canonical cancel text (MySQL family wraps it
                // as "Error: Query cancelled"). What must NOT appear is a raw
                // driver failure: before this was wired up the query surfaced
                // "Error: ORA-01013 ..." as an ordinary error.
                let reported = editor.last_message();
                let looks_like_a_cancel = reported == NO_STATEMENT_RESULT
                    || reported.contains(space_query::db::result_messages::QUERY_CANCELLED);
                if !looks_like_a_cancel {
                    failures.push(format!(
                        "A9 ({label}): a registry cancel surfaced as '{reported}' instead of being reported as a cancel"
                    ));
                } else {
                    println!("   A9 ({label}) reported to the user as a cancel, not a failure");
                }
            }
        }
        let _ = editor.wait_done(Duration::from_secs(10));

    }

    clear_tracked_db_activity();
    Ok(failures)
}

/// Drives a real `SqlEditorWidget` the way the GUI does.
struct EditorHarness {
    editor: SqlEditorWidget,
    done: Arc<AtomicBool>,
    messages: Arc<Mutex<Vec<String>>>,
}

impl EditorHarness {
    fn new(harness: &Harness) -> Self {
        let timeout_input = IntInput::default();
        let mut editor = SqlEditorWidget::new(Arc::clone(&harness.connection), timeout_input);
        let done = Arc::new(AtomicBool::new(false));
        let messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let done = done.clone();
            let messages = messages.clone();
            editor.set_progress_callback(move |event| match progress_inner(&event) {
                // BatchFinished arrives wrapped in Operation/StatementOrigin.
                QueryProgress::BatchFinished => done.store(true, Ordering::Release),
                QueryProgress::StatementFinished { result, .. } => {
                    messages
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(result.message.clone());
                }
                QueryProgress::Message { lines, .. } | QueryProgress::ScriptOutput { lines, .. } => {
                    messages
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .extend(lines.iter().cloned());
                }
                _ => {}
            });
        }
        Self {
            editor,
            done,
            messages,
        }
    }

    fn start(&mut self, sql: &str) {
        self.done.store(false, Ordering::Release);
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.editor.execute_sql_text(sql);
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn wait_done(&self, within: Duration) -> bool {
        self.pump_until(within, || self.is_done().then_some(())).is_some()
    }

    fn run(&mut self, sql: &str, within: Duration) -> bool {
        self.start(sql);
        self.wait_done(within)
    }

    fn retained_session_state(&self) -> Option<space_query::db::PooledSessionLeaseSnapshot> {
        self.editor.pooled_session_activity_snapshot()
    }

    fn last_message(&self) -> String {
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last()
            .cloned()
            .unwrap_or_else(|| NO_STATEMENT_RESULT.to_string())
    }

    /// Pumps the FLTK loop while waiting, like the GUI does — including the
    /// status tick that applies registry-initiated cancels.
    fn pump_until<T>(&self, within: Duration, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.editor.apply_pending_registry_cancel();
            if let Some(value) = ready() {
                return Some(value);
            }
            if !app::wait() {
                app::check();
                thread::sleep(Duration::from_millis(5));
            }
        }
        self.editor.apply_pending_registry_cancel();
        ready()
    }
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. }
        | QueryProgress::StatementOrigin { progress, .. } => progress_inner(progress),
        other => other,
    }
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
            eprintln!("unknown target {other}; use thin|oci|mysql|mariadb|all");
            std::process::exit(2);
        }
    };

    let mut all_failures = Vec::new();
    for target in targets {
        match verify(target) {
            Ok(failures) if failures.is_empty() => println!("== {} PASSED ==", target.label()),
            Ok(failures) => {
                println!("== {} FAILED ==", target.label());
                for failure in &failures {
                    println!("   - {failure}");
                }
                all_failures.extend(failures);
            }
            Err(err) => {
                println!("== {} ERROR == {err}", target.label());
                all_failures.push(format!("{}: {err}", target.label()));
            }
        }
    }

    // The lock-order tracker has been recording throughout every scenario above,
    // which is where the real DB paths run: metadata refresh, execution, retained
    // session reuse, cancels and teardown. Report what it saw.
    all_failures.extend(space_query::db::lock_order::report_observed_lock_order(
        "activity cancel harness",
    ));

    if all_failures.is_empty() {
        println!("\nAll activity cancel scenarios passed.");
    } else {
        println!("\n{} failure(s).", all_failures.len());
        std::process::exit(1);
    }
}
