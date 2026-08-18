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
//   A10 the FORCE tier against work that runs on the connection's OWN session
//       (the explain plan) breaks the call but never DESTROYS the connection
//       every other tab is on. The tier is driven directly, because a graceful
//       break always lands against a real server and the watchdog would never
//       escalate — which is exactly why this hole went unnoticed.
//   A13 a cancel that is STILL TRAVELLING when its session stops being the
//       work's must not reach the server. Every liveness question the app asks
//       is asked before a cancel is dispatched, and on the MySQL family that is
//       a whole control connection away from the `KILL` — which names a server
//       THREAD, so one that arrives late aborts whatever that thread is doing:
//       another tab's statement, or (at the force tier) the session it runs on.
//       Driven with a claim that lapses between the two halves, because that
//       window cannot be reached by waiting.
//   A11 the FORCE tier at the instant a batch GIVES ITS SESSION BACK finds
//       nothing to tear down. The tab's force target and the DB layer's
//       registration used to be given up long after the hand-back — after the
//       progress events and after a runtime read that waits on the shared
//       connection mutex — so a force landing in that window drop-closed the
//       tab's own open transaction (or a session another tab had just taken
//       from the pool). Driven directly for the same reason A10 is, and the
//       assertion is asked of the SERVER: the tab's transaction must still be
//       there afterwards.
//
// Usage: verify_activity_cancel_live <thin|oci|mysql|mariadb|all>

use fltk::{app, input::IntInput};
use space_query::db::{
    active_db_activity_snapshots, active_pool_db_activity_snapshots, cancel_db_activity,
    reset_tracked_db_activities_for_probe, sweep_stale_db_activities, track_pool_db_activity,
    ConnectionInfo, ConnectionRegistry, DatabaseConnection, DatabaseType, DbPoolSession,
    OracleDriverMode, SessionCancelDelivery,
};
use space_query::ui::main_window::MainWindow;
use space_query::ui::sql_editor::{HandBackForceProbe, QueryProgress, SqlEditorWidget};
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
            vec![
                "DROP TABLE SQ_CANCEL_T",
                "CREATE TABLE SQ_CANCEL_T (V NUMBER)",
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_CANCEL_T",
                "CREATE TABLE SQ_CANCEL_T (V INT)",
            ]
        }
    }

    /// What the explain plan under A10 is asked about. The MySQL family's is
    /// the table a second session locks; Oracle's is heavy enough that the
    /// explain is still in flight a moment after it starts.
    fn explain_probe_sql(self) -> &'static str {
        if self.is_oracle() {
            "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c              WHERE a.object_id = b.object_id AND b.object_id = c.object_id"
        } else {
            "SELECT * FROM SQ_CANCEL_T"
        }
    }

    /// The schema A12 loads metadata for.
    ///
    /// It has to hold enough objects that the load spends real time TALKING TO
    /// THE SERVER, because that is the phase A12 is about: the defect it guards
    /// left the activity row cancelable for the session checkout and not for
    /// any of the querying. A load of an empty database is a checkout followed
    /// by almost nothing, and no measurement of it can tell the two apart --
    /// which is what made this scenario report a correct load as the defect on
    /// the MySQL family's small test database.
    ///
    /// Oracle's default (the login schema's view of the dictionary) is already
    /// large; the MySQL family is pointed at `information_schema`, which every
    /// server has and which no test fixture can empty.
    fn metadata_probe_scope(self) -> Option<&'static str> {
        if self.is_oracle() {
            None
        } else {
            Some("information_schema")
        }
    }

    /// A statement every backend can answer instantly, for asking whether the
    /// connection is still there.
    fn trivial_sql(self) -> &'static str {
        if self.is_oracle() {
            "SELECT 1 AS N FROM dual"
        } else {
            "SELECT 1 AS N"
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
                // Session and cancel reach as ONE value: the reach lasts
                // exactly as long as the statement runs on the session.
                let mut acquired = context.acquire_session_for_current_scope(&activity)?;
                acquired_in_worker.store(true, Ordering::Release);
                let Some(session) = acquired.session_mut() else {
                    return Err("the acquired session was already given up".to_string());
                };
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
        if !wait_until(Duration::from_secs(30), || acquired.load(Ordering::Acquire)) {
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

fn run_slow_statement(session: &mut DbPoolSession, sql: &str) -> Result<(), String> {
    match session {
        DbPoolSession::Oracle(conn) => conn
            .query_row(sql, &[])
            .map(|_| ())
            .map_err(|err| err.to_string()),
        DbPoolSession::OracleThin(conn) => conn.query_drop(sql).map_err(|err| err.to_string()),
        DbPoolSession::MySQL { conn, .. } => {
            use mysql::prelude::Queryable;
            conn.as_mut().query_drop(sql).map_err(|err| err.to_string())
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
    reset_tracked_db_activities_for_probe();
    {
        let activity =
            track_pool_db_activity("Live cancel probe", target.connection_info().db_type);
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
            failures
                .push("A1: the probe statement finished on its own; it is not slow enough".into());
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
    reset_tracked_db_activities_for_probe();
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
    reset_tracked_db_activities_for_probe();
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
    reset_tracked_db_activities_for_probe();
    {
        let harness = Harness::connect(target)?;
        let context = harness.pool_context()?;
        let first = track_pool_db_activity("Live detach probe", target.connection_info().db_type);
        {
            let _acquired = context.acquire_session_for_current_scope(&first)?;
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
        reset_tracked_db_activities_for_probe();
        let label = if retained { "A8" } else { "A7" };
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);

        if retained {
            for sql in target.setup_sql() {
                // DROP of a missing table is expected to fail on the first run.
                let _ = editor.run(sql, Duration::from_secs(30));
            }
            if !editor.run(target.retain_session_sql(), Duration::from_secs(30)) {
                failures.push(format!(
                    "{label}: could not open a transaction to retain the session"
                ));
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
                    failures.push(format!(
                        "{label}: the editor query finished before it could be cancelled"
                    ));
                }
                let started = Instant::now();
                if !cancel_db_activity(target_activity.id, PROBE_CANCEL_TIMEOUT) {
                    failures.push(format!(
                        "{label}: cancel_db_activity did not find the editor query"
                    ));
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
                        if retained {
                            "retained-session"
                        } else {
                            "fresh-session"
                        },
                        started.elapsed(),
                        tier_label(started.elapsed())
                    );
                }
                if snapshot_for(target_activity.id).is_some() {
                    failures.push(format!(
                        "{label}: the query's status entry survived the cancel"
                    ));
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

    // A10: cancelling work that runs on the connection's OWN session must not
    // DESTROY that connection. The explain plan is the app's one such
    // operation on every backend, and the query tab's force tier used to reach
    // `terminate()` with no question about which session it spoke for: on the
    // MySQL family that is `KILL CONNECTION` against the app's primary
    // connection — the one every other tab is working on — with nothing
    // marking it disconnected.
    //
    // MySQL family only, and deliberately so: an explain has to be BLOCKED for
    // the force tier to be reached at all, and a metadata lock held by a
    // second session is the one reliable way to do that. Oracle parses an
    // explain without taking a lock any other session can hold, so there is no
    // honest way to stall it here; the rule itself is shared
    // (`CanceledSession::force_tier_may_destroy_it`) and is covered by the
    // unit test and the guard test.
    {
        let label = target.label();
        println!(
            "   A10 ({label}): the force tier must never destroy the connection's own session"
        );
        // A5 ended the outer harness's connection on purpose, so this scenario
        // opens one of its own — like A7/A8 do.
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        editor.editor.set_text(target.explain_probe_sql());
        // A new tab does metadata work of its own on the MAIN connection, and
        // on the MySQL family that work would block on the table lock below
        // while holding the connection — let it finish first.
        editor.pump_until(Duration::from_secs(20), || {
            space_query::db::try_lock_connection(&harness.connection).map(|_| ())
        });

        // Hold the explain still long enough to force it. On the MySQL family a
        // second session's metadata lock does it; Oracle takes no lock another
        // session can hold, so there the explain is caught in flight instead.
        let blocker = if target.is_oracle() {
            None
        } else {
            match block_mysql_table(target) {
                Ok(blocker) => Some(blocker),
                Err(err) => {
                    failures.push(format!(
                        "A10 ({label}): could not take the blocking lock: {err}"
                    ));
                    None
                }
            }
        };
        if target.is_oracle() || blocker.is_some() {
            // The force tier, driven the way the cancel watchdog drives it. It
            // cannot be reached by waiting out a cancel timeout here: every
            // backend's graceful break DOES land, so the watchdog would never
            // escalate — which is exactly why this hole went unnoticed.
            let mut forced = None;
            for _ in 0..40 {
                editor.editor.explain_current();
                let caught = editor.pump_until(Duration::from_secs(3), || {
                    editor.editor.force_cancel_published_session_for_probe()
                });
                if let Some(outcome) = caught {
                    forced = Some(outcome);
                    break;
                }
                editor.pump_until(Duration::from_millis(200), || None::<()>);
            }
            match forced {
                None => failures.push(format!(
                    "A10 ({label}): the explain plan never published a session for the force tier"
                )),
                Some(outcome) => {
                    println!("   A10 ({label}) force tier answered {outcome:?}");
                }
            }
            editor.pump_until(cancel_deadline(), || {
                (!editor.editor.is_query_running()).then_some(())
            });
        }
        // Release the blocker before asking the connection anything.
        if let Some(mut blocker) = blocker {
            use mysql::prelude::Queryable;
            let _ = blocker.query_drop("UNLOCK TABLES");
        }
        editor.pump_until(Duration::from_secs(2), || None::<()>);

        // THE ASSERTION: the connection every other tab is on is still there.
        // Before the force tier asked which session it spoke for, it had just
        // force-closed it (Oracle thin) or `KILL CONNECTION`ed it (MySQL
        // family).
        //
        // Asked with ANOTHER EXPLAIN, deliberately: an ordinary statement runs
        // on a POOLED session and would answer happily over a main connection
        // the app had just destroyed.
        let mut alive = false;
        for _ in 0..5 {
            if editor.explain(target.trivial_sql(), Duration::from_secs(20)) {
                alive = true;
                break;
            }
            editor.pump_until(Duration::from_millis(500), || None::<()>);
        }
        if alive {
            println!("   A10 ({label}) the connection survived the force tier");
        } else {
            failures.push(format!(
                "A10 ({label}): the force tier destroyed the connection every other tab is on \
                 (transcript: {:?})",
                editor.transcript()
            ));
        }
        let _ = editor.wait_done(Duration::from_secs(10));
    }

    // A11: the force tier must find nothing to tear down at a hand-back.
    //
    // The tab is left holding a real open transaction, and the probe then does
    // exactly what a batch does in the same order — take the session, publish
    // it as the operation's cancel target, hand it back through the door — and
    // drives the tier. The door has to have ended the reach by then; if it has
    // not, the tier destroys the session the tab is holding, and the server is
    // asked about that afterwards.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!(
            "   A11 ({label}): a hand-back must end the cancel's reach before the session moves"
        );
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        for sql in target.setup_sql() {
            let _ = editor.run(sql, Duration::from_secs(30));
        }
        if !editor.run(target.retain_session_sql(), Duration::from_secs(30)) {
            failures.push(format!(
                "A11 ({label}): could not open a transaction to retain the session"
            ));
        }
        if editor.retained_session_state().is_none() {
            failures.push(format!(
                "A11 ({label}): the tab did not retain its session, so there is no hand-back to probe"
            ));
        } else {
            match editor.editor.force_the_tier_at_a_hand_back_for_probe() {
                HandBackForceProbe::ReachWithdrawn => {
                    println!("   A11 ({label}) the door ended the reach before the session moved");
                }
                answer => failures.push(format!(
                    "A11 ({label}): the force tier still spoke for a session that had been handed \
                     back: {answer:?}"
                )),
            }

            // THE ASSERTION, asked of the server: the tab's own transaction is
            // still open on a session that still works. Before the door ended
            // the reach first, the tier had just drop-closed (Oracle) or
            // KILLed (MySQL family) exactly this session.
            if editor.retained_session_state().is_none() {
                failures.push(format!(
                    "A11 ({label}): the tab lost its retained session to the force tier"
                ));
            }
            let survived = editor.run(target.retain_session_sql(), Duration::from_secs(30))
                && !editor.last_message().to_ascii_lowercase().contains("error");
            if survived {
                println!("   A11 ({label}) the tab's own transaction survived the force tier");
            } else {
                failures.push(format!(
                    "A11 ({label}): the tab's retained session could not be used after the force \
                     tier (transcript: {:?})",
                    editor.transcript()
                ));
            }
        }
        let _ = editor.wait_done(Duration::from_secs(10));
    }

    // A12: the app's own schema metadata load must stay reachable by the cancel
    // button for as long as it holds a pooled session.
    //
    // This is the longest-running background read the app does, and on every
    // backend it was neither offerable by the cancel button nor breakable by a
    // disconnect: the loader acquired `(session, registration)` and dropped the
    // registration inside a `.map()` before the session was used at all, so the
    // registry entry carried NO canceler while a real server call ran under it.
    // Cancelling it retired the row -- the screen said the work had ended --
    // and broke nothing, so the query ran on holding a pooled session that a
    // disconnect was meanwhile tearing the pool out from under.
    //
    // Two things are asked, because neither alone can see the defect. The
    // registry has to say the load is cancelable for essentially the whole time
    // it holds a session (the defect left it cancelable for the ACQUIRE alone,
    // which is a window a poll can still land in), and a cancel fired inside
    // that window has to actually STOP the load, which is the user-visible
    // half.
    reset_tracked_db_activities_for_probe();
    {
        let label = target.label();
        println!("   A12 ({label}): a schema metadata load must be cancelable while it runs");
        let harness = Harness::connect(target)?;
        // The loader tags its activity with the connection it belongs to, which
        // is what a disconnect matches on, so the probe registers the
        // connection exactly as the app does.
        let registry = ConnectionRegistry::new();
        let _runtime = registry.register_unmanaged(Arc::clone(&harness.connection));
        let expected_activity = harness
            .pool_context()?
            .connection_info
            .db_type
            .metadata_refresh_activity_for_probe(target.metadata_probe_scope());

        // What one bare ACQUIRE costs on this connection right now, measured
        // the same way the load is measured.
        //
        // This is the yardstick, and it has to be measured rather than assumed:
        // the defect left the row cancelable for exactly the acquire and not a
        // moment longer, so "cancelable for meaningfully longer than an acquire"
        // is the property that separates a load whose whole DB phase is
        // reachable from one where only the checkout was. Comparing against the
        // row's TOTAL life instead -- which is what this scenario used to do --
        // measures the wrong thing: the load's tail (rebuilding indices and
        // highlight data) is client-side CPU of roughly constant cost, while
        // the DB phase shrinks by an order of magnitude once the server's
        // caches are warm. That made a correct load look like the defect on any
        // second run, on the baseline as much as on a fix.
        let bare_acquire = {
            let mut longest = Duration::ZERO;
            for _ in 0..3 {
                let context = harness.pool_context()?;
                let probe = context.track_activity("Live acquire yardstick");
                let started = Instant::now();
                {
                    let _session =
                        context.acquire_session_for_scope(target.metadata_probe_scope(), &probe)?;
                }
                longest = longest.max(started.elapsed());
            }
            longest
        };

        // Pass 1: watch a load all the way through and measure how long of the
        // row's life it was cancelable for.
        let context = harness.pool_context()?;
        let done = Arc::new(AtomicBool::new(false));
        let loaded = Arc::new(AtomicBool::new(false));
        let done_in_worker = Arc::clone(&done);
        let loaded_in_worker = Arc::clone(&loaded);
        let probe_scope = target.metadata_probe_scope().map(str::to_string);
        let worker = thread::spawn(move || {
            loaded_in_worker.store(
                MainWindow::load_schema_metadata_for_probe(context, probe_scope),
                Ordering::Release,
            );
            done_in_worker.store(true, Ordering::Release);
        });
        let mut row_polls = 0usize;
        let mut cancelable_polls = 0usize;
        let mut cancelable_run = 0usize;
        let mut longest_cancelable_run = 0usize;
        // The same observation in TIME, which is what the yardstick above is in.
        let mut cancelable_since: Option<Instant> = None;
        let mut longest_cancelable_span = Duration::ZERO;
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline && !done.load(Ordering::Acquire) {
            if let Some(row) = active_pool_db_activity_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.activity == expected_activity)
            {
                row_polls += 1;
                if row.cancelable {
                    cancelable_polls += 1;
                    cancelable_run += 1;
                    longest_cancelable_run = longest_cancelable_run.max(cancelable_run);
                    let since = *cancelable_since.get_or_insert_with(Instant::now);
                    longest_cancelable_span = longest_cancelable_span.max(since.elapsed());
                } else {
                    cancelable_run = 0;
                    cancelable_since = None;
                }
            }
        }
        let _ = worker.join();
        // TWO measurements, and the load has to satisfy EITHER.
        //
        // Neither is sound on its own, and each is unsound on a different
        // family:
        //
        //  * "cancelable for most of the row's life" breaks on Oracle. The
        //    load's tail -- rebuilding indices and highlight data over the
        //    metadata it just read -- is client-side CPU of roughly constant
        //    cost, while the DB phase shrinks by an order of magnitude once the
        //    server's caches are warm. Measured here on the same build: 81% on
        //    a cold database and 37% on the next run. That is a correct load
        //    reported as the defect, on the baseline as much as on a fix.
        //
        //  * "cancelable for meaningfully longer than a bare checkout" breaks on
        //    the MySQL family, whose checkout is several round trips (reset,
        //    `USE`, session settings) while its metadata read is two cheap
        //    `information_schema` queries: 11.9ms of reach against a 9.3ms
        //    checkout, which is healthy and looks marginal.
        //
        // What makes the pair sound is what the DEFECT does to both. It dropped
        // the registration in the closure that acquired the session, so the row
        // was cancelable only for the instant between the attach at the END of
        // the checkout and that drop -- not for the checkout, and not for any of
        // the querying. Measured against a deliberately reintroduced defect:
        // 0.8% of the row's life, and 469us against a 49ms checkout. It fails
        // both by two orders of magnitude, so requiring either one keeps the
        // whole of the sensitivity and neither of the false failures.
        const MINIMUM_ACQUIRE_MULTIPLE: u32 = 3;
        let required = bare_acquire * MINIMUM_ACQUIRE_MULTIPLE;
        let reachable_for_most_of_its_life = cancelable_polls * 2 >= row_polls;
        let reachable_for_longer_than_a_checkout = longest_cancelable_span >= required;
        println!(
            "   A12 ({label}) metadata load: cancelable for {longest_cancelable_span:?} \
             (a bare acquire takes {bare_acquire:?}); seen on {cancelable_polls} of {row_polls} \
             observations (longest unbroken run {longest_cancelable_run}), produced metadata: {}",
            loaded.load(Ordering::Acquire)
        );
        if row_polls == 0 {
            failures.push(format!(
                "A12 ({label}): the schema metadata load never published an activity row, so \
                 the probe could not observe it (expected {expected_activity:?})"
            ));
        } else if !reachable_for_most_of_its_life && !reachable_for_longer_than_a_checkout {
            failures.push(format!(
                "A12 ({label}): the schema metadata load was reachable for \
                 {longest_cancelable_span:?} -- {cancelable_polls} of {row_polls} observations, \
                 and no longer than the {bare_acquire:?} its session checkout takes -- so it \
                 held a pooled session the cancel button could not offer and a disconnect could \
                 not break"
            ));
        }
        if !loaded.load(Ordering::Acquire) {
            failures.push(format!(
                "A12 ({label}): the schema metadata load produced nothing, so the scenario \
                 proved nothing about a load that works"
            ));
        }

        // Pass 2: and the reach is real -- a cancel fired while the load holds
        // its session stops it, rather than only clearing the status bar.
        //
        // Reported, not required. Both tiers run on the watchdog thread and the
        // MySQL family's graceful break opens a SECOND connection to issue
        // `KILL QUERY`, so on a small test database the load can simply finish
        // first. That says nothing about the defect -- pass 1 is what sees it
        // -- so a load that outran its own cancel is a note, not a failure.
        let mut stopped = false;
        let mut cancelled_a_row = false;
        for _ in 0..4 {
            let context = harness.pool_context()?;
            let done = Arc::new(AtomicBool::new(false));
            let loaded = Arc::new(AtomicBool::new(false));
            let done_in_worker = Arc::clone(&done);
            let loaded_in_worker = Arc::clone(&loaded);
            let probe_scope = target.metadata_probe_scope().map(str::to_string);
            let worker = thread::spawn(move || {
                loaded_in_worker.store(
                    MainWindow::load_schema_metadata_for_probe(context, probe_scope),
                    Ordering::Release,
                );
                done_in_worker.store(true, Ordering::Release);
            });
            let deadline = Instant::now() + Duration::from_secs(180);
            while Instant::now() < deadline && !done.load(Ordering::Acquire) {
                // Cancel only what the registry says is REACHABLE. A cancel
                // that lands before the canceler is attached is refused by the
                // acquire itself, which stops the load whether or not the
                // session it goes on to hold is reachable.
                if let Some(row) = active_pool_db_activity_snapshots()
                    .into_iter()
                    .find(|snapshot| snapshot.activity == expected_activity && snapshot.cancelable)
                {
                    cancelled_a_row |= cancel_db_activity(row.id, PROBE_CANCEL_TIMEOUT);
                    break;
                }
            }
            let _ = worker.join();
            if cancelled_a_row && !loaded.load(Ordering::Acquire) {
                stopped = true;
                break;
            }
        }
        if !cancelled_a_row {
            failures.push(format!(
                "A12 ({label}): the schema metadata load never offered a cancelable row, so the \
                 cancel button had nothing to reach"
            ));
        } else if stopped {
            println!("   A12 ({label}) and cancelling it actually stopped the load");
        } else {
            println!(
                "   A12 ({label}) note: every load finished before its own cancel could land \
                 (the break runs on the watchdog thread), so only the reach was checked"
            );
        }
    }

    // A13: a cancel that is still on its way when its session stops being the
    // work's must reach nothing.
    //
    // The app asks "is this still our session?" before it DISPATCHES a cancel.
    // On both Oracle drivers the answer and the effect are microseconds apart;
    // on the MySQL family they are a whole control connection apart — TCP
    // connect, handshake, auth — and a `KILL` names a server THREAD, so one
    // that arrives after the session went back to the pool aborts whichever tab
    // picked it up. The window cannot be reached by waiting, so it is reached
    // by SAYING when the session stopped being the work's: the probe's claim
    // answers yes once (the cancel really was aimed here when it was
    // dispatched) and no from then on.
    //
    // The assertion is asked of the SERVER: the statement the cancel was aimed
    // at must run to completion, because nothing was sent.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!(
            "   A13 ({label}): a cancel that is still travelling when its session moves must \
             reach nothing"
        );
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        // No setup table: this scenario only needs a statement that keeps
        // running, and `slow_sql` reads the data dictionary. Creating the
        // shared probe table here would queue behind the open transaction A11
        // deliberately leaves on it.
        // A SCRIPT, so the slow statement keeps the operation's own session
        // rather than handing it to a lazy fetch: a single-statement SELECT
        // goes down the lazy road, which publishes its own withdrawable target
        // and leaves the operation slot withdrawn. The window this scenario is
        // about is the operation's.
        editor.start_script(&format!("{};\n{}", target.slow_sql(), target.trivial_sql()));
        let probed = editor.pump_until(Duration::from_secs(30), || {
            editor
                .editor
                .cancel_published_session_with_a_lapsing_claim_for_probe()
        });
        match probed {
            None => failures.push(format!(
                "A13 ({label}): the query never published a session, so the window could not \
                 be probed (transcript: {:?})",
                editor.transcript()
            )),
            Some(Err(message)) => failures.push(format!(
                "A13 ({label}): a cancel whose session had moved on answered a FAILURE the user \
                 would be invited to retry: {message}"
            )),
            Some(Ok(SessionCancelDelivery::Delivered)) => failures.push(format!(
                "A13 ({label}): the cancel reached the server after its session stopped being \
                 this work's -- on the MySQL family that is a KILL against whichever tab holds \
                 that session now"
            )),
            Some(Ok(SessionCancelDelivery::Withdrawn)) => {
                println!("   A13 ({label}) nothing was sent");
            }
        }
        // And the statement itself must be UNTOUCHED. `slow_sql` runs far
        // longer than any assertion window on purpose, so it is not waited out;
        // what is asked instead is the pair of facts a landed cancel would have
        // broken. First: after a settle window long enough for a `KILL` to have
        // arrived, the statement is still running.
        editor.pump_until(Duration::from_secs(3), || None::<()>);
        if editor.is_done() {
            failures.push(format!(
                "A13 ({label}): the statement ended anyway (reported: {}), so something reached \
                 the server after its session stopped being this work's",
                editor.last_message()
            ));
        } else {
            println!("   A13 ({label}) and the statement it was aimed at kept running");
        }

        // Second: the session is still THERE and still this tab's -- a real
        // cancel, down the ordinary road, still lands on it. On the MySQL
        // family the force tier of the same cancel is `KILL CONNECTION`, so a
        // session that had been destroyed would answer this with a connection
        // error instead of a cancel.
        editor.editor.cancel_current();
        if editor.wait_done(Duration::from_secs(60)) {
            let reported = editor.last_message();
            let looks_like_a_cancel = reported == NO_STATEMENT_RESULT
                || space_query::db::session_policy::message_indicates_query_cancel(&reported);
            if looks_like_a_cancel {
                println!("   A13 ({label}) and its session was still reachable afterwards");
            } else {
                failures.push(format!(
                    "A13 ({label}): the session the lapsed cancel left alone could not be \
                     cancelled afterwards (reported: {reported})"
                ));
            }
        } else {
            failures.push(format!(
                "A13 ({label}): the statement could not be ended afterwards, so its session \
                 may not have survived"
            ));
        }
    }

    reset_tracked_db_activities_for_probe();
    Ok(failures)
}

/// A second MySQL-family session holding a metadata lock on the probe table, so
/// an `EXPLAIN` of it blocks long enough for the force tier to be reached.
fn block_mysql_table(target: Target) -> Result<mysql::Conn, String> {
    use mysql::prelude::Queryable;

    let info = target.connection_info();
    let url = format!(
        "mysql://{}:{}@{}:{}/{}",
        info.username, info.password, info.host, info.port, info.service_name
    );
    let mut conn = mysql::Conn::new(url.as_str()).map_err(|err| err.to_string())?;
    conn.query_drop("LOCK TABLES SQ_CANCEL_T WRITE")
        .map_err(|err| err.to_string())?;
    Ok(conn)
}

/// Drives a real `SqlEditorWidget` the way the GUI does.
struct EditorHarness {
    editor: SqlEditorWidget,
    done: Arc<AtomicBool>,
    messages: Arc<Mutex<Vec<String>>>,
    explained: Arc<AtomicBool>,
}

impl EditorHarness {
    fn new(harness: &Harness) -> Self {
        let timeout_input = IntInput::default();
        let mut editor = SqlEditorWidget::new(Arc::clone(&harness.connection), timeout_input);
        let done = Arc::new(AtomicBool::new(false));
        let messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let explained = Arc::new(AtomicBool::new(false));
        {
            let done = done.clone();
            let messages = messages.clone();
            let explained = explained.clone();
            editor.set_progress_callback(move |event| match progress_inner(&event) {
                // The explain plan's own success: it is the app's one operation
                // that runs on the connection's OWN session, so it is also how
                // this harness asks whether that session is still there.
                QueryProgress::ExplainPlanOutput { .. } => explained.store(true, Ordering::Release),
                // BatchFinished arrives wrapped in Operation/StatementOrigin.
                QueryProgress::BatchFinished => done.store(true, Ordering::Release),
                QueryProgress::StatementFinished { result, .. } => {
                    messages
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(result.message.clone());
                }
                QueryProgress::Message { lines, .. }
                | QueryProgress::ScriptOutput { lines, .. } => {
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
            explained,
        }
    }

    /// Run an explain plan and wait for the server's answer. The explain is the
    /// app's one MAIN-connection operation, so this is how the harness asks
    /// whether that connection is still usable.
    fn explain(&mut self, sql: &str, within: Duration) -> bool {
        self.explained.store(false, Ordering::Release);
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.editor.set_text(sql);
        self.editor.explain_current();
        self.pump_until(within, || {
            self.explained.load(Ordering::Acquire).then_some(())
        })
        .is_some()
    }

    fn start(&mut self, sql: &str) {
        self.done.store(false, Ordering::Release);
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.editor.execute_sql_text(sql);
    }

    /// Run several statements as one batch, the way F5 does.
    ///
    /// A single-statement SELECT is handed to a LAZY FETCH, which publishes its
    /// own withdrawable target and leaves the operation's slot withdrawn; a
    /// script keeps the session on the operation for its whole run.
    fn start_script(&mut self, sql: &str) {
        self.done.store(false, Ordering::Release);
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.editor.execute_script_for_harness(sql);
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn wait_done(&self, within: Duration) -> bool {
        self.pump_until(within, || self.is_done().then_some(()))
            .is_some()
    }

    fn run(&mut self, sql: &str, within: Duration) -> bool {
        self.start(sql);
        self.wait_done(within)
    }

    fn retained_session_state(&self) -> Option<space_query::db::PooledSessionLeaseSnapshot> {
        self.editor.pooled_session_activity_snapshot()
    }

    fn transcript(&self) -> Vec<String> {
        self.messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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
