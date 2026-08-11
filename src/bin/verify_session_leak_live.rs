#![allow(clippy::cargo, clippy::pedantic)]

// Live: no query-tab lifecycle event leaves a session open on the SERVER.
//
// The lib-level engine (`assert_connection_lifecycle_closes_every_server_session`)
// proves this for the lease layer. It cannot see the holders that only exist
// once a real tab is running: the lazy-fetch worker owns its session in its own
// frame rather than in the tab's lease slot, and a cancelled statement leaves
// the session in whatever state the force close left it.
//
//   T1 closing a tab closes the session the tab was holding
//   T2 closing a tab closes the session an OPEN lazy fetch is holding
//   T3 closing a tab closes the session a cancelled statement was using
//   T4 repeated tab open/close leaves nothing behind
//   T5 disconnecting closes the session of a statement still on the server
//   T6 disconnecting closes every session the tab ever used
//
// Every count is the server's own (`information_schema.processlist` /
// `v$session`), taken through a second connection, and the probe connects
// under an identity of its own (a database of its own on the MySQL family, a
// user of its own on Oracle) so the count sees this harness and nobody else.
//
// Usage: verify_session_leak_live <thin|oci|mysql|mariadb|all>
// Run one database container at a time.

use fltk::{app, input::IntInput};
use mysql::prelude::Queryable;
use space_query::db::{ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode};
use space_query::ui::sql_editor::{LazyFetchRequest, QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

const PROBE_NAME: &str = "sq_session_probe_ui";
const PROBE_ORACLE_PASSWORD: &str = "sq_probe_2026";

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

    fn host(self) -> String {
        if self.is_oracle() {
            env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into())
        } else {
            "127.0.0.1".into()
        }
    }

    fn port(self) -> u16 {
        match self {
            Target::OracleThin | Target::OracleOci => env::var("ORACLE_TEST_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1521),
            Target::MySql => 3307,
            Target::MariaDb => 3306,
        }
    }

    fn service(self) -> String {
        match self {
            Target::OracleThin | Target::OracleOci => env::var("ORACLE_TEST_SERVICE_NAME")
                .or_else(|_| env::var("ORACLE_TEST_SERVICE"))
                .unwrap_or_else(|_| "FREE".into()),
            Target::MySql => "query_tool_mysql8".into(),
            Target::MariaDb => "query_tool_test".into(),
        }
    }

    fn admin_user(self) -> String {
        if self.is_oracle() {
            env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into())
        } else {
            "root".into()
        }
    }

    fn admin_password(self) -> String {
        match self {
            Target::OracleThin | Target::OracleOci => {
                env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".into())
            }
            Target::MySql => "spacequery".into(),
            Target::MariaDb => "password".into(),
        }
    }

    fn db_type(self) -> DatabaseType {
        match self {
            Target::OracleThin | Target::OracleOci => DatabaseType::Oracle,
            Target::MySql => DatabaseType::MySQL,
            Target::MariaDb => DatabaseType::MariaDB,
        }
    }

    /// The connection the tabs use: the same server, reached through the
    /// probe's own identity.
    fn probe_connection_info(self, probe_user: &str, probe_service: &str) -> ConnectionInfo {
        let (user, password) = if self.is_oracle() {
            (probe_user.to_string(), PROBE_ORACLE_PASSWORD.to_string())
        } else {
            (self.admin_user(), self.admin_password())
        };
        let mut info = ConnectionInfo::new_with_type(
            "session leak probe",
            &user,
            &password,
            &self.host(),
            self.port(),
            probe_service,
            self.db_type(),
        );
        if self.is_oracle() {
            info.advanced.oracle_driver_mode = if self == Target::OracleThin {
                OracleDriverMode::Thin
            } else {
                OracleDriverMode::Oci
            };
        }
        info
    }

    /// A statement that stays on the server long enough to be cancelled.
    fn sleep_sql(self) -> &'static str {
        if self.is_oracle() {
            "BEGIN DBMS_SESSION.SLEEP(30); END;"
        } else {
            "SELECT SLEEP(30)"
        }
    }

    /// A row source big enough to leave a lazy fetch open, without needing a
    /// table (the Oracle probe user owns nothing).
    fn many_rows_sql(self) -> &'static str {
        if self.is_oracle() {
            "SELECT LEVEL AS N FROM dual CONNECT BY LEVEL <= 20000"
        } else {
            "SELECT a.ORDINAL_POSITION AS N FROM information_schema.COLUMNS a, information_schema.COLUMNS b LIMIT 20000"
        }
    }
}

/// The server's own session count for the probe identity.
enum Census {
    MySqlFamily {
        conn: mysql::Conn,
        database: String,
    },
    OracleOci {
        conn: oracle::Connection,
        user: String,
    },
    OracleThin {
        session: Box<OracleThinSession>,
        user: String,
    },
}

impl Census {
    fn count(&mut self) -> Result<usize, String> {
        match self {
            Census::MySqlFamily { conn, database } => {
                let sql = format!(
                    "SELECT COUNT(*) FROM information_schema.processlist WHERE db = '{}'",
                    database.replace('\'', "''")
                );
                let count: Option<i64> = conn.query_first(sql).map_err(|err| err.to_string())?;
                Ok(count.unwrap_or_default().max(0) as usize)
            }
            Census::OracleOci { conn, user } => {
                let count: i64 = conn
                    .query_row_as(
                        "SELECT COUNT(*) FROM v$session WHERE username = :1",
                        &[&user.to_uppercase()],
                    )
                    .map_err(|err| err.to_string())?;
                Ok(count.max(0) as usize)
            }
            Census::OracleThin { session, user } => {
                let sql = format!(
                    "SELECT COUNT(*) FROM v$session WHERE username = '{}'",
                    user.to_uppercase().replace('\'', "''")
                );
                let described = session
                    .query_described_fetch_all(sql, 1)
                    .map_err(|err| err.to_string())?;
                let value = match described.result.rows.first().and_then(|row| row.first()) {
                    Some(tns_thin::exec::OracleValue::Number(text)) => text.clone(),
                    Some(tns_thin::exec::OracleValue::Text(text)) => text.clone(),
                    other => return Err(format!("unexpected session count value: {other:?}")),
                };
                Ok(value.trim().parse::<i64>().unwrap_or_default().max(0) as usize)
            }
        }
    }
}

/// Poll until the count comes down to the limit. Closing a session is not
/// always synchronous — the worker has to notice the cancel, the server has to
/// notice the FIN — so what matters is where it settles.
fn settled_count(census: &mut Census, limit: usize) -> Result<usize, String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut observed = census.count()?;
    while observed > limit && Instant::now() < deadline {
        // Keep the UI loop alive: the app closes sessions from callbacks and
        // worker completions that only run while events are pumped.
        pump(Duration::from_millis(200));
        observed = census.count()?;
    }
    Ok(observed)
}

fn stable_count(census: &mut Census) -> Result<usize, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut previous = census.count()?;
    loop {
        pump(Duration::from_millis(400));
        let observed = census.count()?;
        if observed == previous || Instant::now() >= deadline {
            return Ok(observed);
        }
        previous = observed;
    }
}

/// Run the UI loop for a while. The app closes sessions from callbacks and
/// from worker completions that only run while events are pumped, so every
/// wait in this harness has to keep the loop alive rather than sleep.
fn pump(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if app::wait_for(0.01).is_err() {
            std::thread::sleep(Duration::from_millis(5));
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

/// One query tab, driven the way the app drives it.
struct Tab {
    editor: SqlEditorWidget,
    done: Arc<AtomicBool>,
}

impl Tab {
    fn open(shared: &Arc<Mutex<DatabaseConnection>>) -> Self {
        let timeout_input = IntInput::default();
        let mut editor = SqlEditorWidget::new(Arc::clone(shared), timeout_input);
        let done = Arc::new(AtomicBool::new(false));
        {
            let done = Arc::clone(&done);
            editor.set_progress_callback(move |event| {
                if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                    done.store(true, Ordering::SeqCst);
                }
            });
        }
        Self { editor, done }
    }

    fn run(&mut self, sql: &str) -> Result<(), String> {
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        self.wait_for_batch(Duration::from_secs(60))
    }

    fn start(&mut self, sql: &str) {
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
    }

    fn wait_for_batch(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while !self.done.load(Ordering::SeqCst) && Instant::now() < deadline {
            pump(Duration::from_millis(10));
        }
        if !self.done.load(Ordering::SeqCst) {
            return Err("timed out waiting for the statement to finish".to_string());
        }
        pump(Duration::from_millis(250));
        Ok(())
    }

    /// Exactly what closing the tab does in main_window: cancel and discard
    /// every open lazy fetch, then clean the editor up.
    fn close(&mut self) {
        if let Some(session_id) = self.editor.active_lazy_fetch_session() {
            self.editor
                .request_lazy_fetch(session_id, LazyFetchRequest::CancelAndDiscard);
        }
        self.editor.cleanup_for_close();
        pump(Duration::from_millis(250));
    }
}

struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, label: &str, observed: usize, limit: usize) {
        if observed <= limit {
            println!("    OK  {label} (server sessions: {observed} <= {limit})");
        } else {
            println!("    FAIL {label}: {observed} sessions still open, expected at most {limit}");
            self.failures.push(label.to_string());
        }
    }
}

fn oracle_probe_user(target: Target) -> Result<(String, Census), String> {
    let host = target.host();
    let port = target.port();
    let service = target.service();
    let admin_user = target.admin_user();
    let admin_password = target.admin_password();
    let mut user = PROBE_NAME.to_uppercase();

    // The census talks to the server as the admin user; the probe gets a user
    // of its own so the count belongs to this harness alone.
    let mut census = if target == Target::OracleThin {
        let config = OracleThinConfig::new(
            ConnectTarget::service_name(host.clone(), port, service.clone()),
            admin_user.clone(),
            admin_password.clone(),
        );
        let session = OracleThinSession::connect(config)
            .map_err(|err| format!("connect the Oracle thin census: {err}"))?;
        Census::OracleThin {
            session: Box::new(session),
            user: user.clone(),
        }
    } else {
        let conn = oracle::Connection::connect(
            &admin_user,
            &admin_password,
            format!("//{host}:{port}/{service}"),
        )
        .map_err(|err| format!("connect the Oracle OCI census: {err}"))?;
        Census::OracleOci {
            conn,
            user: user.clone(),
        }
    };

    let mut execute = |sql: &str| -> Result<(), String> {
        match &mut census {
            Census::OracleThin { session, .. } => {
                session.query_drop(sql).map_err(|err| err.to_string())
            }
            Census::OracleOci { conn, .. } => conn
                .execute(sql, &[])
                .map(|_| ())
                .map_err(|err| err.to_string()),
            Census::MySqlFamily { .. } => Err("not an Oracle census".to_string()),
        }
    };

    let create =
        |user: &str| format!("CREATE USER {user} IDENTIFIED BY \"{PROBE_ORACLE_PASSWORD}\"");
    if let Err(err) = execute(&create(&user)) {
        let message = err.to_ascii_lowercase();
        if message.contains("ora-65096") {
            // A CDB root only accepts common user names.
            user = format!("C##{user}");
            if let Err(err) = execute(&create(&user)) {
                if !err.to_ascii_lowercase().contains("ora-01920") {
                    return Err(format!("create the Oracle probe user: {err}"));
                }
            }
        } else if !message.contains("ora-01920") {
            return Err(format!("create the Oracle probe user: {err}"));
        }
    }
    for grant in [
        format!("GRANT CREATE SESSION TO {user}"),
        format!("GRANT SELECT ANY DICTIONARY TO {user}"),
    ] {
        execute(&grant).map_err(|err| format!("{grant}: {err}"))?;
    }

    match &mut census {
        Census::OracleThin { user: name, .. } | Census::OracleOci { user: name, .. } => {
            *name = user.clone();
        }
        Census::MySqlFamily { .. } => {}
    }
    Ok((user, census))
}

fn mysql_probe_database(target: Target) -> Result<Census, String> {
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(target.host()))
        .tcp_port(target.port())
        .user(Some(target.admin_user()))
        .pass(Some(target.admin_password()))
        .db_name(Some(target.service()));
    let mut conn = mysql::Conn::new(opts).map_err(|err| format!("connect the census: {err}"))?;
    conn.query_drop(format!("CREATE DATABASE IF NOT EXISTS {PROBE_NAME}"))
        .map_err(|err| format!("create the probe database: {err}"))?;
    Ok(Census::MySqlFamily {
        conn,
        database: PROBE_NAME.to_string(),
    })
}

fn connection_reconnect(
    shared: &Arc<Mutex<DatabaseConnection>>,
    target: Target,
    probe_user: &str,
    probe_service: &str,
) -> Result<(), String> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connect(target.probe_connection_info(probe_user, probe_service))
        .map_err(|err| format!("reconnect: {err}"))
}

fn verify(target: Target) -> Result<bool, String> {
    println!("\n########## {} ##########", target.label());

    let (probe_user, probe_service, mut census) = if target.is_oracle() {
        let (user, census) = oracle_probe_user(target)?;
        (user, target.service(), census)
    } else {
        (
            target.admin_user(),
            PROBE_NAME.to_string(),
            mysql_probe_database(target)?,
        )
    };
    let info = target.probe_connection_info(&probe_user, &probe_service);

    let disconnected_baseline = stable_count(&mut census)?;
    println!("  baseline (nothing connected): {disconnected_baseline}");

    let mut connection = DatabaseConnection::new();
    connection
        .connect(info)
        .map_err(|err| format!("connect: {err}"))?;
    let shared = Arc::new(Mutex::new(connection));
    let connected_baseline = stable_count(&mut census)?;
    println!("  baseline (connected, no tab work): {connected_baseline}");

    let mut report = Report { failures: vec![] };

    // T1: the plain case — a tab that ran a statement holds a session.
    println!("  --- T1 closing a tab closes its retained session ---");
    let mut tab = Tab::open(&shared);
    tab.run("SELECT 1 AS ONE FROM dual")
        .or_else(|_| tab.run("SELECT 1 AS ONE"))?;
    let working = stable_count(&mut census)?;
    if working <= connected_baseline {
        println!("    (note) the tab did not open a session of its own: {working}");
    }
    tab.close();
    let observed = settled_count(&mut census, connected_baseline)?;
    report.check(
        "T1 closing a tab closes its retained session",
        observed,
        connected_baseline,
    );

    // T2: the lazy-fetch worker owns its session outside the tab's lease slot.
    println!("  --- T2 closing a tab closes an open lazy fetch's session ---");
    let mut tab = Tab::open(&shared);
    tab.editor.set_lazy_fetch_batch_size(50);
    tab.run(target.many_rows_sql())?;
    let open = tab.editor.has_open_lazy_fetch();
    println!("    lazy fetch open after the SELECT: {open}");
    if !open {
        println!("    (note) no lazy fetch stayed open; T2 degenerates into T1");
    }
    tab.close();
    let observed = settled_count(&mut census, connected_baseline)?;
    report.check(
        "T2 closing a tab closes an open lazy fetch's session",
        observed,
        connected_baseline,
    );

    // A script CONNECT's own connection is deliberately NOT checked here. It
    // is torn down by dropping the tab's binding, and a harness cannot destroy
    // the FLTK widget that holds it, so the connection would stay alive for
    // reasons that have nothing to do with the product. That teardown is
    // proven where it can be driven exactly: `L9` in the lib-level engine
    // drops a connection without disconnecting it and requires the same
    // guarantee.

    // T3: a cancelled statement leaves the session wherever the force close
    // left it. Closing the tab still has to close it.
    println!("  --- T3 closing a tab closes a cancelled statement's session ---");
    let mut tab = Tab::open(&shared);
    tab.start(target.sleep_sql());
    pump(Duration::from_millis(1500));
    tab.editor.cancel_current();
    let _ = tab.wait_for_batch(Duration::from_secs(60));
    tab.close();
    let observed = settled_count(&mut census, connected_baseline)?;
    report.check(
        "T3 closing a tab closes a cancelled statement's session",
        observed,
        connected_baseline,
    );

    // T4: tabs opened and closed over and over must not accumulate. A leak of
    // one session per tab is invisible in a single pass and fatal in a day's
    // work.
    println!("  --- T4 repeated tab open/close leaves nothing behind ---");
    let mut worst = 0usize;
    for round in 0..5 {
        let mut tab = Tab::open(&shared);
        tab.run("SELECT 1 AS ONE FROM dual")
            .or_else(|_| tab.run("SELECT 1 AS ONE"))
            .map_err(|err| format!("round {round}: {err}"))?;
        tab.close();
        worst = worst.max(settled_count(&mut census, connected_baseline)?);
    }
    report.check(
        "T4 five tab open/close rounds leave nothing behind",
        worst,
        connected_baseline,
    );

    // T5: disconnecting while a statement is still on the server. The session
    // is in the worker's own frame, not in any lease slot, so nothing the
    // teardown can reach owns it -- only the stale sweep the app runs on every
    // disconnect can retire that work and take the session with it.
    println!("  --- T5 disconnecting closes an in-flight statement's session ---");
    let mut tab = Tab::open(&shared);
    tab.start(target.sleep_sql());
    pump(Duration::from_millis(1500));
    let in_flight = census.count()?;
    println!("    sessions while the statement is running: {in_flight}");
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .disconnect();
    // Exactly what main_window does after a disconnect.
    space_query::db::sweep_stale_db_activities(Duration::from_secs(2));
    let _ = tab.wait_for_batch(Duration::from_secs(60));
    tab.close();
    let observed = settled_count(&mut census, disconnected_baseline)?;
    report.check(
        "T5 disconnecting closes an in-flight statement's session",
        observed,
        disconnected_baseline,
    );

    // T6: and the plain disconnect takes everything with it.
    println!("  --- T6 disconnecting closes every session the tab used ---");
    connection_reconnect(&shared, target, &probe_user, &probe_service)?;
    let mut tab = Tab::open(&shared);
    tab.run("SELECT 1 AS ONE FROM dual")
        .or_else(|_| tab.run("SELECT 1 AS ONE"))?;
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .disconnect();
    tab.close();
    let observed = settled_count(&mut census, disconnected_baseline)?;
    report.check(
        "T6 disconnecting closes every session the tab used",
        observed,
        disconnected_baseline,
    );

    if report.failures.is_empty() {
        println!("== {} PASSED ==", target.label());
        Ok(false)
    } else {
        println!("== {} FAILED: {:?} ==", target.label(), report.failures);
        Ok(true)
    }
}

fn main() {
    let arg = env::args().nth(1).unwrap_or_else(|| "all".to_string());
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
    let mut failed = false;
    for target in targets {
        match verify(target) {
            Ok(target_failed) => failed |= target_failed,
            Err(err) => {
                println!("== {} ERROR: {err} ==", target.label());
                failed = true;
            }
        }
    }
    if failed {
        println!("\nSESSION LEAK CHECKS FAILED");
        std::process::exit(1);
    }
    println!("\nALL SESSION LEAK CHECKS PASSED");
}
