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
//   A18 cancelling a PAUSED lazy fetch asks the server nothing, so the
//       watchdog's force pass is the only thing that can ever ask it — the
//       premise the lazy force tier used to escalate on, checked per backend.
//   A19 the SAME publication, forced with the purpose that ENDS THE
//       CONNECTION, must reach the tier that destroys — never the re-break
//       A10 asserts for a cancel. Without it the app could not end a call on
//       the connection's own session at all, so `File > Disconnect` refused on
//       a statement the app had already told the user it could not stop.
//   A16 (Oracle only — the MySQL family refuses `CONNECT`) a script `CONNECT`
//       moves the operation's REGISTRY ROW to the connection the batch moved
//       to. Only the row's connection ID used to move; its lifetime kept
//       naming the connection the batch had LEFT, and that connection's own
//       teardown gate no longer refuses (the tab is bound elsewhere now), so
//       ending it made the row stale and the stale sweep — which a disconnect
//       runs on the spot — cancelled the batch running somewhere else.
//
// Usage: verify_activity_cancel_live <thin|oci|mysql|mariadb|all>

use fltk::prelude::InputExt;
use fltk::{app, input::IntInput};
use space_query::db::{
    active_db_activity_snapshots, active_pool_db_activity_snapshots, cancel_db_activity,
    reset_tracked_db_activities_for_probe, resize_shared_connection_pool,
    sweep_stale_db_activities, track_pool_db_activity, ConnectionInfo, ConnectionRegistry,
    DatabaseConnection, DatabaseType, DbPoolSession, OracleDriverMode, SessionCancelDelivery,
};
use space_query::ui::main_window::MainWindow;
use space_query::ui::sql_editor::{
    HandBackForceProbe, MainSessionTargetAtLockRelease, QueryProgress, SqlEditorWidget,
};
use std::collections::HashSet;
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

    /// A14's own probe table.
    ///
    /// Deliberately NOT the shared one: A11 leaves an open transaction on
    /// `SQ_CANCEL_T` on purpose, so creating or writing it here would queue
    /// behind that lock rather than testing anything. A13 says the same.
    fn timeout_setup_sql(self) -> Vec<&'static str> {
        if self.is_oracle() {
            vec![
                "DROP TABLE SQ_TIMEOUT_T",
                "CREATE TABLE SQ_TIMEOUT_T (V NUMBER)",
            ]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_TIMEOUT_T",
                "CREATE TABLE SQ_TIMEOUT_T (V INT)",
            ]
        }
    }

    /// A17's own probe table.
    ///
    /// Its own, and not A14's, for a reason the harness cannot avoid: a
    /// scenario that ends with the tab still holding an open transaction keeps
    /// holding it (a harness cannot destroy the FLTK widget that owns the
    /// lease), and on the MySQL family that transaction holds a METADATA lock
    /// the next scenario's `DROP TABLE` waits behind — long enough to
    /// desynchronise everything after it.
    fn kill_setup_sql(self) -> Vec<&'static str> {
        if self.is_oracle() {
            vec!["DROP TABLE SQ_KILL_T", "CREATE TABLE SQ_KILL_T (V NUMBER)"]
        } else {
            vec![
                "DROP TABLE IF EXISTS SQ_KILL_T",
                "CREATE TABLE SQ_KILL_T (V INT)",
            ]
        }
    }

    fn kill_retain_sql(self) -> &'static str {
        if self.is_oracle() {
            "INSERT INTO SQ_KILL_T VALUES (1)"
        } else {
            "START TRANSACTION; INSERT INTO SQ_KILL_T VALUES (1)"
        }
    }

    fn timeout_retain_sql(self) -> &'static str {
        if self.is_oracle() {
            "INSERT INTO SQ_TIMEOUT_T VALUES (1)"
        } else {
            "START TRANSACTION; INSERT INTO SQ_TIMEOUT_T VALUES (1)"
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

    /// A SELECT whose result is far larger than one lazy-fetch batch, so the
    /// fetch stops after its first chunk and WAITS for the user -- the state
    /// A18 is about, and the one every result grid sits in after a big query.
    fn paused_lazy_select_sql(self) -> &'static str {
        if self.is_oracle() {
            "SELECT a.OBJECT_NAME, b.OBJECT_NAME AS B_NAME FROM all_objects a, all_objects b              WHERE a.object_id > 0"
        } else {
            "SELECT a.COLUMN_NAME, b.COLUMN_NAME AS B_NAME FROM information_schema.COLUMNS a,              information_schema.COLUMNS b"
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
                let mut acquired = context.acquire_session_for_current_scope(
                    space_query::db::PooledSessionPurpose::AppRead,
                    &activity,
                )?;
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

/// Every server session holding an open transaction EXCEPT this one, as
/// `sid,serial#`.
///
/// Its own exclusion is not tidiness. The killer takes its session from the
/// same pool the tabs use, and a pooled Oracle session is recycled WITHOUT a
/// rollback — so a session an earlier scenario left holding a transaction can
/// be handed straight back to the killer, which then disconnects itself
/// half-way through the loop and reports `DPI-1010: not connected` for whatever
/// it tried to kill next. "From a session of its own" has to be true of the
/// query as well as of the connection.
const ORACLE_OPEN_TRANSACTION_SESSIONS: &str = "SELECT s.sid || ',' || s.serial# FROM v$transaction t JOIN v$session s ON s.saddr = t.ses_addr WHERE s.sid <> SYS_CONTEXT('USERENV', 'SID')";

/// DISCONNECT rather than KILL, because the session is IN a call: `KILL
/// SESSION` answers ORA-00031 ("marked for kill") and leaves the call running,
/// which is the opposite of what this scenario needs — the connection has to go
/// while the statement is on it. `IMMEDIATE` rolls the transaction back and
/// ends the session without waiting for the call to finish.
fn oracle_disconnect_session_sql(session: &str) -> String {
    format!("ALTER SYSTEM DISCONNECT SESSION '{session}' IMMEDIATE")
}

/// Oracle answers ORA-00031 when it could only MARK the session; the session
/// still dies, so the scenario's kill has been delivered.
fn oracle_kill_was_delivered(error: &str) -> bool {
    error.contains("ORA-00031")
}

/// How many rounds the kill loop below may take before it gives up. One
/// session dies per round, and the scenarios that lead here leave at most a
/// handful behind.
const KILL_ROUNDS: usize = 16;

/// Kill EVERY server session that is holding an open transaction, one per
/// round, each round from a killer session of its own.
///
/// With one scenario running that is exactly the query tab's retained session,
/// and the user's uncommitted work is on it — which is what makes the kill a
/// deterministic stand-in for the connection dying under a running statement.
/// Answers what it killed, so a scenario can print it.
///
/// A fresh killer per round, and the list re-read every round, because on
/// Oracle the killer cannot be sure of surviving its own kill: it takes its
/// session from the same pool the tabs use, and disconnecting a sibling of that
/// pool takes the killer's own checked-out session down with it (`DPI-1010: not
/// connected`). Killing the whole list from one session therefore lost the tool
/// half-way through and left the rest of the list alive — intermittently, since
/// which session is a sibling depends on what the earlier scenarios left
/// behind. Losing the killer is harmless when it is rebuilt anyway, and the
/// re-read is what makes a round that died before the server acted simply
/// happen again.
fn kill_the_session_holding_the_open_transaction(target: Target) -> Result<String, String> {
    let mut killed: Vec<String> = Vec::new();
    // Asked ONCE each. Oracle can answer ORA-00031 — "marked for kill" — for a
    // session that is still in `v$transaction` on the next read, and without
    // this the loop asked the same session every round until it ran out.
    let mut already_asked: HashSet<String> = HashSet::new();
    let mut last_error: Option<String> = None;
    for _ in 0..KILL_ROUNDS {
        match kill_one_session_holding_an_open_transaction(target, &mut already_asked) {
            Ok(None) => {
                last_error = None;
                break;
            }
            Ok(Some(session)) => {
                killed.push(session);
                last_error = None;
            }
            // The killer died with its own kill, or could not be built this
            // round. The list is re-read next round, so a session the server
            // never got to is simply seen again.
            Err(message) => last_error = Some(message),
        }
    }
    if let Some(message) = last_error {
        return Err(format!(
            "gave up after {KILL_ROUNDS} rounds (killed {}): {message}",
            if killed.is_empty() {
                "nothing".to_string()
            } else {
                killed.join(",")
            }
        ));
    }
    if killed.is_empty() {
        return Err("no open transaction to kill".to_string());
    }
    Ok(killed.join(","))
}

/// One round: read the list, kill the first entry, answer which one — or
/// `None` when nothing is left holding a transaction.
fn kill_one_session_holding_an_open_transaction(
    target: Target,
    already_asked: &mut HashSet<String>,
) -> Result<Option<String>, String> {
    let killer = Harness::connect(target)?;
    let context = killer.pool_context()?;
    let activity = track_pool_db_activity("A17 kill", target.connection_info().db_type);
    let mut acquired = context.acquire_session_for_current_scope(
        space_query::db::PooledSessionPurpose::AppRead,
        &activity,
    )?;
    let Some(session) = acquired.session_mut() else {
        return Err("the killer session was already given up".to_string());
    };
    match session {
        DbPoolSession::Oracle(conn) => {
            let rows = conn
                .query_as::<String>(ORACLE_OPEN_TRANSACTION_SESSIONS, &[])
                .map_err(|err| format!("v$transaction: {err}"))?;
            let mut open = Vec::new();
            for row in rows {
                open.push(row.map_err(|err| format!("v$transaction row: {err}"))?);
            }
            let Some(target_session) = open
                .iter()
                .find(|session| !already_asked.contains(*session))
                .cloned()
            else {
                return Ok(None);
            };
            already_asked.insert(target_session.clone());
            if let Err(err) = conn.execute(&oracle_disconnect_session_sql(&target_session), &[]) {
                let message = err.to_string();
                if !oracle_kill_was_delivered(&message) {
                    return Err(format!("kill {target_session}: {message}"));
                }
            }
            Ok(Some(target_session))
        }
        DbPoolSession::OracleThin(conn) => {
            let described = conn
                .query_described_fetch_all(ORACLE_OPEN_TRANSACTION_SESSIONS.to_string(), 64)
                .map_err(|err| format!("v$transaction: {err}"))?;
            let mut open = Vec::new();
            for row in &described.result.rows {
                match row.first() {
                    Some(tns_thin::exec::OracleValue::Text(text)) => open.push(text.clone()),
                    Some(tns_thin::exec::OracleValue::Number(text)) => open.push(text.clone()),
                    other => return Err(format!("unexpected session identity: {other:?}")),
                }
            }
            let Some(target_session) = open
                .iter()
                .find(|session| !already_asked.contains(*session))
                .cloned()
            else {
                return Ok(None);
            };
            already_asked.insert(target_session.clone());
            if let Err(err) = conn.query_drop(&oracle_disconnect_session_sql(&target_session)) {
                let message = err.to_string();
                if !oracle_kill_was_delivered(&message) {
                    return Err(format!("kill {target_session}: {message}"));
                }
            }
            Ok(Some(target_session))
        }
        DbPoolSession::MySQL { conn, .. } => {
            use mysql::prelude::Queryable;
            // Its own thread excluded for the reason the Oracle query gives:
            // the killer's session comes from the same pool the tabs use, and
            // killing it mid-loop leaves the rest unkilled.
            let ids: Vec<u64> = conn
                .as_mut()
                .query(
                    "SELECT trx_mysql_thread_id FROM information_schema.innodb_trx \
                     WHERE trx_mysql_thread_id <> CONNECTION_ID()",
                )
                .map_err(|err| format!("innodb_trx: {err}"))?;
            let Some(id) = ids
                .iter()
                .find(|id| !already_asked.contains(&id.to_string()))
                .copied()
            else {
                return Ok(None);
            };
            already_asked.insert(id.to_string());
            conn.as_mut()
                .query_drop(format!("KILL {id}"))
                .map_err(|err| format!("kill {id}: {err}"))?;
            Ok(Some(id.to_string()))
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
            let _acquired = context.acquire_session_for_current_scope(
                space_query::db::PooledSessionPurpose::AppRead,
                &first,
            )?;
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
                //
                // The loss notice is NOT the statement's answer and must not be
                // read as one: when the cancel's force tier closes a session
                // carrying the user's work, "that work is gone" is a SECOND
                // fact the app owes — asserted for A8 below — and it arrives on
                // the same transcript.
                let transcript = editor.transcript();
                let loss = space_query::db::result_messages::RETAINED_SESSION_LOST_WITH_WORK;
                let reported = transcript
                    .iter()
                    .rev()
                    .find(|line| !line.contains(loss))
                    .cloned()
                    .unwrap_or_else(|| NO_STATEMENT_RESULT.to_string());
                let looks_like_a_cancel = reported == NO_STATEMENT_RESULT
                    || reported.contains(space_query::db::result_messages::QUERY_CANCELLED);
                if !looks_like_a_cancel {
                    failures.push(format!(
                        "A9 ({label}): a registry cancel surfaced as '{reported}' instead of being reported as a cancel"
                    ));
                } else {
                    println!("   A9 ({label}) reported to the user as a cancel, not a failure");
                }
                // A8 only: the tab went into this holding an open transaction,
                // so the cancel leaves it in one of exactly two stated
                // conditions — the session and the work still there (tier 1
                // ended only the statement), or the session gone AND the loss
                // reported. Silence is what this refuses, and silence is what
                // it used to get: the close is decided by the interrupt policy,
                // which closed the session without reading what it was
                // carrying.
                if retained {
                    let still_holds = editor.retained_session_state().is_some_and(|snapshot| {
                        snapshot.retained_state.may_have_uncommitted_work()
                    });
                    if still_holds {
                        println!("   A9 ({label}) the cancel cost the statement and left the work");
                    } else if transcript.iter().any(|line| line.contains(loss)) {
                        println!(
                            "   A9 ({label}) the session went with the cancel, and the user was \
                             told the work went with it"
                        );
                    } else {
                        failures.push(format!(
                            "A9 ({label}): the cancel closed a work-carrying session and nothing \
                             said so (transcript: {})",
                            transcript.join(" | ")
                        ));
                    }
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
                    editor.editor.force_cancel_published_session_for_probe(
                        space_query::db::SessionCancelPurpose::StopOneCall,
                    )
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
                // The tier must NAME what it did, and against the connection's
                // own session the only honest name is "I broke it again".
                // While both tiers answered `Delivered`, the cancel watchdog
                // read this as the tear-down: it reported `ForceCompleted`,
                // retired the operation's activity row and abandoned the
                // operation -- publishing the tab idle and clearing the cancel
                // flag that stops a batch at its next safe point -- for a
                // statement the server was still running. On all four backends.
                Some(Ok(space_query::db::ForceTierOutcome::AskedAgain)) => {
                    println!(
                        "   A10 ({label}) the force tier broke the connection's own session \
                         again and said so, rather than claiming a tear-down"
                    );
                }
                Some(Ok(space_query::db::ForceTierOutcome::Destroyed)) => failures.push(format!(
                    "A10 ({label}): the force tier reported DESTROYING the connection's own \
                     session -- the one every other tab is working on"
                )),
                Some(Ok(space_query::db::ForceTierOutcome::Withdrawn)) => failures.push(format!(
                    "A10 ({label}): the explain plan's session was withdrawn before the tier \
                     could reach it, so this scenario proved nothing"
                )),
                Some(Err(message)) => failures.push(format!(
                    "A10 ({label}): the force tier failed against the connection's own session: \
                     {message}"
                )),
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

    // A15: a cancel target naming the connection's OWN session ends with the
    // LOCK that makes that session exclusively this tab's.
    //
    // A pooled session is given up at a hand-back door, and A11 above proves
    // that door ends the reach first. The connection's own session has no such
    // door -- what makes it one caller's is the connection MUTEX -- so the
    // mutex is the door. It was not: the Oracle explain plan publishes the main
    // session on both drivers and cleared the tab's target only AFTER its guard
    // had been dropped. In that window (the mutex free, the target still
    // naming the session) another tab takes the connection and starts its own
    // main-connection call, and a cancel of the FINISHED explain breaks THAT
    // call -- `break_execution` on the shared Oracle session, `KILL QUERY` on
    // the shared MySQL-family one. The MySQL family escaped it only because its
    // one main-connection execution path happened to clear its context before
    // returning; happening to is not a rule, and it did not survive a panic.
    //
    // Driven directly for the same reason A10 and A11 are: the window is a
    // handful of instructions wide and cannot be reached by waiting.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!(
            "   A15 ({label}): a main-session cancel target must end with the lock that owns it"
        );
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        // A fresh tab does metadata work on the MAIN connection; let it finish
        // so the probe's own lock is not refused as busy.
        editor.pump_until(Duration::from_secs(20), || {
            space_query::db::try_lock_connection(&harness.connection).map(|_| ())
        });
        match editor
            .editor
            .main_session_cancel_target_at_lock_release_for_probe(&harness.connection)
        {
            MainSessionTargetAtLockRelease::WithdrawnWithTheLock => {
                println!(
                    "   A15 ({label}) the lock ended the target it published over its own session"
                );
            }
            answer => failures.push(format!(
                "A15 ({label}): the tab's cancel still named the connection's own session after \
                 the lock was released: {answer:?}"
            )),
        }

        // THE ASSERTION, asked of the server: the connection every other tab is
        // on still works. Asked with an EXPLAIN, like A10, because an ordinary
        // statement runs on a pooled session and would answer happily over a
        // main connection that had just been broken.
        let mut alive = false;
        for _ in 0..5 {
            if editor.explain(target.trivial_sql(), Duration::from_secs(20)) {
                alive = true;
                break;
            }
            editor.pump_until(Duration::from_millis(500), || None::<()>);
        }
        if alive {
            println!("   A15 ({label}) and the connection it named is untouched");
        } else {
            failures.push(format!(
                "A15 ({label}): publishing and withdrawing a main-session target broke the \
                 connection (transcript: {:?})",
                editor.transcript()
            ));
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
                    let _session = context.acquire_session_for_scope(
                        target.metadata_probe_scope(),
                        space_query::db::PooledSessionPurpose::AppRead,
                        &probe,
                    )?;
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

    // A14: a query timeout costs the STATEMENT, and the session's fate is the
    // same stated answer on all four backends.
    //
    // Oracle OCI breaks and resets inside ODPI-C (`DPI-1067`) and the MySQL
    // family's timeout is server-side (`ERROR 3024`), so on both the tab keeps
    // its session and its open transaction. Oracle thin had neither: its call
    // timeout was a bare socket read timeout that left the server's answer
    // pending on the wire, and its interrupt classifier had no timeout arm at
    // all, so the timeout was reported as a LOST CONNECTION and the tab's
    // session -- with the user's uncommitted work on it -- was replaced without
    // a word.
    //
    // The contract this asserts is the one all four now obey: the failure names
    // a TIMEOUT, and the tab is left in one of exactly two stated conditions --
    // its session and transaction intact, or the session gone AND the loss
    // reported.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!("   A14 ({label}): a query timeout costs the statement, and says what else");
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        for sql in target.timeout_setup_sql() {
            let _ = editor.run(sql, Duration::from_secs(30));
        }
        if !editor.run(target.timeout_retain_sql(), Duration::from_secs(30)) {
            failures.push(format!(
                "A14 ({label}): could not open a transaction to retain the session"
            ));
        }
        let retained_before = editor.retained_session_state().is_some();
        let carried_work_before = editor
            .retained_session_state()
            .is_some_and(|snapshot| snapshot.retained_state.may_have_uncommitted_work());
        if !retained_before || !carried_work_before {
            failures.push(format!(
                "A14 ({label}): the tab did not retain a session carrying work, so there is \
                 nothing for a timeout to cost"
            ));
        }

        editor.set_query_timeout_seconds(2);
        // As a SCRIPT, for the same reason A13 does it: a single-statement
        // SELECT is handed to a lazy fetch, and this is about the statement
        // road the tab's own session runs on.
        //
        // Pumped until the tab has SAID something rather than waiting on
        // `BatchFinished`: a terminal event queued by the previous statement is
        // drained by the same poll, so the flag can already be set when this
        // batch starts.
        editor.start(&format!("{};\n{}", target.slow_sql(), target.trivial_sql()));
        let reported = editor.pump_until(Duration::from_secs(60), || {
            let transcript = editor.transcript();
            (!transcript.is_empty()).then(|| transcript.join(" | "))
        });
        let _ = editor.wait_done(Duration::from_secs(20));
        editor.set_query_timeout_seconds(0);
        let transcript = reported.unwrap_or_default();
        let lowered = transcript.to_ascii_lowercase();
        if transcript.is_empty() {
            failures.push(format!(
                "A14 ({label}): the timed-out statement told the user nothing at all"
            ));
        } else if !(lowered.contains("timed out")
            || lowered.contains("timeout")
            || lowered.contains("ora-01013"))
        {
            failures.push(format!(
                "A14 ({label}): a query timeout must be reported as a timeout, not as something \
                 else (transcript: {transcript})"
            ));
        }

        // The two stated conditions. Either is correct; SILENCE is not.
        if editor.retained_session_state().is_some() {
            let usable = editor.run(target.timeout_retain_sql(), Duration::from_secs(30))
                && !editor.last_message().to_ascii_lowercase().contains("error");
            if usable {
                println!(
                    "   A14 ({label}) the session survived the timeout and the tab's transaction \
                     is still open"
                );
            } else {
                failures.push(format!(
                    "A14 ({label}): the tab kept a session it cannot use (transcript: {:?})",
                    editor.transcript()
                ));
            }
        } else if carried_work_before {
            if lowered.contains(
                &space_query::db::result_messages::RETAINED_SESSION_LOST_WITH_WORK
                    .to_ascii_lowercase(),
            ) {
                println!(
                    "   A14 ({label}) the session could not be resynchronised, and the user was \
                     told the work went with it"
                );
            } else {
                failures.push(format!(
                    "A14 ({label}): the tab's work-carrying session was closed by a timeout and \
                     nothing said so (transcript: {transcript})"
                ));
            }
        }
        let _ = editor.wait_done(Duration::from_secs(10));
    }

    // A17: a work-carrying session that DIES while its statement is running
    // must tell the user the work went with it.
    //
    // This is the CLEANUP's road, not the acquisition's, and that is the whole
    // point of the scenario. When the session is found dead at the NEXT
    // acquisition the app has always reported the loss; when it dies mid-
    // statement the close is decided by the interrupt policy instead —
    // `ReplacePhysicalSessionKeepUiConnected` on Oracle (answered for a
    // connection error before the retained state is even looked at) and the
    // MySQL family's own disposition — and BOTH closed the session in silence.
    // The Oracle arm had the state in hand and never read it; the MySQL arm
    // could not read it at all, because the outcome value it was given carried
    // no state to read.
    //
    // The kill is the deterministic way to reach it on all four backends: the
    // statement fails as a connection error, which is exactly what a real
    // force-cancel, a server restart or a network drop produce.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!("   A17 ({label}): a session that dies mid-statement says what it took with it");
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        // Settle on the tab being IDLE between steps, not on the done flag: a
        // terminal event queued by the previous statement is drained by the
        // same poll, so `run` can return while the batch is still going — and
        // the next `execute_sql_text` is then refused outright, silently
        // skipping the step. (A14 lives with it because only its final wait
        // matters; this scenario has to get its transaction open first.)
        fn settle(editor: &EditorHarness) {
            editor.pump_until(Duration::from_secs(30), || {
                (!editor.editor.is_query_running()).then_some(())
            });
        }
        for sql in target.kill_setup_sql() {
            settle(&editor);
            let _ = editor.run(sql, Duration::from_secs(30));
        }
        settle(&editor);
        let _ = editor.run(target.kill_retain_sql(), Duration::from_secs(30));
        // Waited for as a FACT, not from the done flag: a terminal event queued
        // by the previous statement is drained by the same poll, so the flag
        // can already be set when this batch starts (A14 says the same).
        let carried_work_before = editor
            .pump_until(Duration::from_secs(30), || {
                editor
                    .retained_session_state()
                    .is_some_and(|snapshot| snapshot.retained_state.may_have_uncommitted_work())
                    .then_some(())
            })
            .is_some();
        if !carried_work_before {
            failures.push(format!(
                "A17 ({label}): the tab did not retain a session carrying work, so there is \
                 nothing for the kill to take"
            ));
        }
        // As a SCRIPT, for the reason A13 and A14 give: a single-statement
        // SELECT is handed to a lazy fetch, and this is about the statement
        // road the tab's own session runs on.
        settle(&editor);
        editor.start(&format!("{};\n{}", target.slow_sql(), target.trivial_sql()));
        // The kill has to land on a session that is really executing, or it
        // becomes the acquisition's road again.
        thread::sleep(Duration::from_millis(800));
        match kill_the_session_holding_the_open_transaction(target) {
            Ok(killed) => println!("   A17 ({label}) killed the tab's session ({killed})"),
            Err(message) => failures.push(format!("A17 ({label}): {message}")),
        }
        let reported = editor.pump_until(Duration::from_secs(60), || {
            let transcript = editor.transcript();
            (!transcript.is_empty()).then(|| transcript.join(" | "))
        });
        let _ = editor.wait_done(Duration::from_secs(30));
        let transcript = reported.unwrap_or_default();
        let lowered = transcript.to_ascii_lowercase();
        // The same two stated conditions A14 asserts, and for the same reason:
        // either the tab still holds a session it can use, or the session is
        // gone AND the loss was reported. SILENCE is what this scenario refuses.
        if editor
            .retained_session_state()
            .is_some_and(|snapshot| snapshot.retained_state.may_have_uncommitted_work())
        {
            // The tab still CLAIMS the work — which is what both Oracle drivers
            // answer here, because an interrupted statement with work on the
            // session takes the resolution road rather than the replace one.
            // The claim is only honest if something still settles it: the next
            // statement finds the session dead, and THAT is where the loss has
            // to be said. A claim that simply disappears is the same silence by
            // a longer road.
            println!(
                "   A17 ({label}) the tab still holds the claim; the next statement settles it"
            );
            settle(&editor);
            editor.start(target.trivial_sql());
            let after = editor
                .pump_until(Duration::from_secs(60), || {
                    let transcript = editor.transcript();
                    (!transcript.is_empty()).then(|| transcript.join(" | "))
                })
                .unwrap_or_default();
            let _ = editor.wait_done(Duration::from_secs(30));
            let still_claims = editor
                .retained_session_state()
                .is_some_and(|snapshot| snapshot.retained_state.may_have_uncommitted_work());
            if after.to_ascii_lowercase().contains(
                &space_query::db::result_messages::RETAINED_SESSION_LOST_WITH_WORK
                    .to_ascii_lowercase(),
            ) {
                println!("   A17 ({label}) and the next statement answered the loss");
            } else if still_claims {
                println!(
                    "   A17 ({label}) and the tab still holds the work for the user to resolve"
                );
            } else {
                failures.push(format!(
                    "A17 ({label}): the tab's claim on the work disappeared without a word \
                     (transcript: {after})"
                ));
            }
        } else if lowered.contains(
            &space_query::db::result_messages::RETAINED_SESSION_LOST_WITH_WORK.to_ascii_lowercase(),
        ) {
            println!("   A17 ({label}) the user was told the work went with the session");
        } else {
            failures.push(format!(
                "A17 ({label}): the tab's work-carrying session was closed and nothing said so \
                 (transcript: {transcript})"
            ));
        }
        let _ = editor.run("COMMIT", Duration::from_secs(30));
    }

    // A16: a script CONNECT moves the operation's REGISTRY ROW with it.
    //
    // The registry keeps three facts about which connection a row belongs to:
    // the id a teardown matches on, the lifetime the stale sweep asks, and the
    // generation the cancel hook filters for. Only the id used to move, so the
    // row went on naming the connection the batch had LEFT. That connection's
    // own teardown gate no longer refuses -- the tab is bound elsewhere now --
    // so ending it made the row stale, and the stale sweep a disconnect runs on
    // the spot cancelled the batch running on the connection it moved TO.
    //
    // Oracle only: the MySQL family refuses `CONNECT` outright, so it has no
    // road that moves a running batch between connections.
    if target.is_oracle() {
        let label = target.label();
        println!(
            "   A16 ({label}): ending the connection a script LEFT does not cancel the batch \
             it moved to"
        );
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        let info = target.connection_info();
        let script = format!(
            "CONNECT {}/{}@{}:{}/{}\n{};",
            info.username,
            info.password,
            info.host,
            info.port,
            info.service_name,
            target.slow_sql()
        );
        editor.start_script(&script);

        // Wait for the batch to be PAST the CONNECT: from here it is running on
        // the candidate connection, and the row must have come with it.
        let connected = editor
            .pump_until(Duration::from_secs(60), || {
                editor
                    .transcript()
                    .iter()
                    .any(|line| line.contains("Connected to"))
                    .then_some(())
            })
            .is_some();
        if !connected {
            failures.push(format!(
                "A16 ({label}): the script never reported a CONNECT, so there is no move to \
                 probe (transcript: {:?})",
                editor.transcript()
            ));
        } else if editor.is_done() {
            failures.push(format!(
                "A16 ({label}): the batch ended before the probe could run (transcript: {:?})",
                editor.transcript()
            ));
        } else {
            // End the incarnation of the connection the batch LEFT. A pool
            // rebuild is the production road that does exactly this while the
            // connection itself stays up, and it is the one the harness can
            // drive.
            if let Err(err) = resize_shared_connection_pool(&harness.connection, 6, 30) {
                failures.push(format!(
                    "A16 ({label}): could not retire the connection the script left: {err}"
                ));
            }
            // The sweep a disconnect runs on the spot, and the UI tick runs
            // every frame.
            sweep_stale_db_activities(PROBE_CANCEL_TIMEOUT);
            let ended = editor
                .pump_until(Duration::from_secs(10), || editor.is_done().then_some(()))
                .is_some();
            let transcript = editor.transcript().join(" | ");
            if ended {
                failures.push(format!(
                    "A16 ({label}): the batch was ended by a teardown of the connection it had \
                     already left (transcript: {transcript})"
                ));
            } else {
                println!("   A16 ({label}) the batch kept running on the connection it moved to");
            }

            // Clean up: stop the statement, and prove the session it is on can
            // still be reached -- the other half of the same defect, where the
            // row named a connection whose teardown could break nothing.
            editor.editor.cancel_current();
            if !editor.wait_done(cancel_deadline()) {
                failures.push(format!(
                    "A16 ({label}): the batch could not be cancelled afterwards, so its row no \
                     longer reaches the session it runs on (transcript: {})",
                    editor.transcript().join(" | ")
                ));
            }
        }
    }

    // A18: cancelling a PAUSED lazy fetch asks the server nothing, so the
    // watchdog's force pass is the only thing that can ever ask it.
    //
    // This is the PREMISE the lazy road's force tier used to act on, checked
    // per backend because it is a per-backend fact.
    // `cancel_lazy_fetch_handle_for_session` sends a DB break only for a fetch
    // that is MID-FILL; a fetch that is waiting between chunks -- which is
    // where every result grid sits after a big query, and what every
    // result-tab close and every paused cancel button reaches -- is sent a
    // `GracefulClose` and nothing else. Its publication therefore stays at
    // `GracefulBreakProgress::NotAsked` for the whole close, and the watchdog
    // used to read that exactly as it read `Answered`: escalate. A close that
    // wedged (a cursor close behind a lock, a stalled socket) met KILL
    // CONNECTION / a drop-close as the first thing that ever reached the
    // session -- destroying the tab's own retained transaction that `Cancel`
    // had just promised to keep.
    //
    // What the watchdog now DOES with `NotAsked` is proven by the unit
    // `a_paused_lazy_fetchs_session_is_asked_to_stop_before_it_is_torn_down`
    // (fail-before): the wedge itself cannot be arranged against a healthy
    // server, which is the same split rounds 8, 20 and 21 used.
    {
        use space_query::db::session_policy::LazyFetchState;
        use space_query::ui::sql_editor::{GracefulBreakProgress, LazyFetchRequest};

        let label = target.label();
        println!(
            "   A18 ({label}): cancelling a paused lazy fetch reaches the server only through \
             the watchdog"
        );
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        editor.editor.set_text(target.paused_lazy_select_sql());
        editor.editor.execute_current();

        let paused = editor.pump_until(Duration::from_secs(60), || {
            let snapshot = editor.editor.cancel_target_snapshot();
            (snapshot.lazy_state == LazyFetchState::Waiting
                && editor.editor.active_lazy_fetch_session().is_some())
            .then(|| editor.editor.active_lazy_fetch_session())
            .flatten()
        });
        match paused {
            None => failures.push(format!(
                "A18 ({label}): the query never left a lazy fetch waiting between chunks, so \
                 the premise could not be checked (transcript: {:?})",
                editor.transcript()
            )),
            Some(session_id) => {
                // `Cancel`, not `CancelAndDiscard`: the road that promises to
                // KEEP the tab's session.
                let requested = editor
                    .editor
                    .request_lazy_fetch(session_id, LazyFetchRequest::Cancel);
                if !requested {
                    failures.push(format!(
                        "A18 ({label}): the paused lazy fetch refused the cancel"
                    ));
                }
                let mut asked_the_server = None;
                let closed = editor.pump_until(Duration::from_secs(60), || {
                    match editor.editor.lazy_fetch_graceful_break_progress_for_probe() {
                        Some(GracefulBreakProgress::NotAsked) | None => {}
                        Some(progress) => asked_the_server = Some(progress),
                    }
                    editor
                        .editor
                        .active_lazy_fetch_session()
                        .is_none()
                        .then_some(())
                });
                if closed.is_none() {
                    failures.push(format!("A18 ({label}): the lazy fetch never closed"));
                }
                match asked_the_server {
                    None => println!(
                        "   A18 ({label}) nothing asked the server to stop, so the force tier \
                         would have been the first thing to reach this session"
                    ),
                    Some(progress) => println!(
                        "   A18 ({label}) the cancel road did break the session ({progress:?}); \
                         the premise no longer holds on this backend, and the watchdog's own \
                         break is then simply redundant"
                    ),
                }
                // And the promise the `Cancel` made: the tab's session is still
                // usable afterwards.
                if !editor.run(target.trivial_sql(), Duration::from_secs(30))
                    || editor.last_message().to_ascii_lowercase().contains("error")
                {
                    failures.push(format!(
                        "A18 ({label}): the tab could not use its session after a retaining \
                         cancel of a paused lazy fetch (transcript: {:?})",
                        editor.transcript()
                    ));
                }
            }
        }
        let _ = editor.wait_done(Duration::from_secs(10));
    }

    // A19: the OTHER half of the rule A10 asserts. The same publication -- the
    // explain plan's, which runs on the connection's OWN session on all four
    // backends -- forced with the purpose that ENDS THE CONNECTION must reach
    // the tier that destroys, never the re-break.
    //
    // Why it matters, and why it is live rather than only a unit: the rule used
    // to be a fact about the SESSION alone, which read as "a main session is
    // never destroyed" and left the deliberate action the rule's own header
    // names -- File > Disconnect -- unable to destroy one either. A statement
    // wedged there (Oracle thin's in-band break does not reach a call that is
    // already blocked) could then be neither cancelled nor disconnected around:
    // the force tier told the user to disconnect, and the disconnect answered
    // "Stop it before continuing".
    //
    // The assertion is "NOT AskedAgain", which is exactly the branch this
    // change opens. What the destroy itself answers is a per-driver fact and is
    // printed rather than asserted: OCI cannot drop-close a connection with a
    // call in flight (DPI-1011), and there the graceful tier -- which does land
    // on OCI -- is what ends the work; the app's own wait
    // (`wait_until_ended_db_work_let_go`) is what decides either way.
    //
    // Destructive by design: it opens a connection of its own and does not use
    // it afterwards.
    {
        reset_tracked_db_activities_for_probe();
        let label = target.label();
        println!("   A19 ({label}): the tier that ENDS the connection may destroy its own session");
        let harness = Harness::connect(target)?;
        let mut editor = EditorHarness::new(&harness);
        editor.editor.set_text(target.explain_probe_sql());
        editor.pump_until(Duration::from_secs(20), || {
            space_query::db::try_lock_connection(&harness.connection).map(|_| ())
        });
        let blocker = if target.is_oracle() {
            None
        } else {
            match block_mysql_table(target) {
                Ok(blocker) => Some(blocker),
                Err(err) => {
                    failures.push(format!(
                        "A19 ({label}): could not take the blocking lock: {err}"
                    ));
                    None
                }
            }
        };
        if target.is_oracle() || blocker.is_some() {
            let mut forced = None;
            for _ in 0..40 {
                editor.editor.explain_current();
                let caught = editor.pump_until(Duration::from_secs(3), || {
                    editor.editor.force_cancel_published_session_for_probe(
                        space_query::db::SessionCancelPurpose::EndTheConnection,
                    )
                });
                if let Some(outcome) = caught {
                    forced = Some(outcome);
                    break;
                }
                editor.pump_until(Duration::from_millis(200), || None::<()>);
            }
            match forced {
                None => failures.push(format!(
                    "A19 ({label}): the explain plan never published a session for the force tier"
                )),
                Some(Ok(space_query::db::ForceTierOutcome::AskedAgain)) => failures.push(format!(
                    "A19 ({label}): the action that ENDS the connection was still refused the                      tier that destroys, so a statement wedged on the connection's own session                      can be neither cancelled nor disconnected around"
                )),
                Some(Ok(space_query::db::ForceTierOutcome::Destroyed)) => println!(
                    "   A19 ({label}) the connection-ending tier tore its own session down"
                ),
                Some(Ok(space_query::db::ForceTierOutcome::Withdrawn)) => failures.push(format!(
                    "A19 ({label}): the explain plan's session was withdrawn before the tier                      could reach it, so this scenario proved nothing"
                )),
                Some(Err(message)) => println!(
                    "   A19 ({label}) the tier that destroys was REACHED and this driver                      refused it ({message}); the graceful tier is what ends the work here, and                      the app's own wait decides"
                ),
            }
            editor.pump_until(cancel_deadline(), || {
                (!editor.editor.is_query_running()).then_some(())
            });
        }
        if let Some(mut blocker) = blocker {
            use mysql::prelude::Queryable;
            let _ = blocker.query_drop("UNLOCK TABLES");
        }
        let _ = editor.wait_done(Duration::from_secs(10));
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
    /// The tab's query-timeout field, kept so a scenario can set one.
    timeout_input: IntInput,
    done: Arc<AtomicBool>,
    messages: Arc<Mutex<Vec<String>>>,
    explained: Arc<AtomicBool>,
}

impl EditorHarness {
    fn new(harness: &Harness) -> Self {
        let timeout_input = IntInput::default();
        let mut editor =
            SqlEditorWidget::new(Arc::clone(&harness.connection), timeout_input.clone());
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
                // The lost-session notice has a variant of its own (the
                // window drops an abandoned operation's messages, and an
                // abandoned operation is exactly when a worker's session is
                // closed), and a scenario asking what the user was told has to
                // read it where it now travels.
                QueryProgress::Message { lines, .. }
                | QueryProgress::RetainedSessionLostWithWork { lines }
                | QueryProgress::ScriptOutput { lines, .. } => {
                    messages
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .extend(lines.iter().cloned());
                }
                // A batch that DIED says so here and nowhere else, so a
                // scenario asking what the user was told has to see it.
                QueryProgress::ExecutionAbandoned { message, .. }
                | QueryProgress::WorkerPanicked { message } => {
                    messages
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(message.clone());
                }
                _ => {}
            });
        }
        Self {
            editor,
            timeout_input,
            done,
            messages,
            explained,
        }
    }

    /// Give this tab a query timeout, the way the toolbar field does.
    fn set_query_timeout_seconds(&mut self, seconds: u32) {
        self.timeout_input.set_value(&seconds.to_string());
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
