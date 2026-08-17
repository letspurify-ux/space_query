#![allow(clippy::cargo, clippy::pedantic)]

// Live verification of the read-only connection guard (item 11) on every
// supported backend: Oracle Thin, Oracle OCI, MySQL and MariaDB.
//
// The unit tests in `src/db/sql_classification.rs` settle which statements the
// guard classifies as reads. What only a server can settle is the part the
// feature exists for: that a refused statement leaves the database untouched.
// A guard that classified correctly but still sent the statement would pass
// every unit test and protect nothing.
//
// So each write below is attempted through the production execution path —
// `SqlEditorWidget::execute_sql_text`, what Ctrl+Enter calls — and then the row
// count is read back over a *second, writable* connection to the same database.
// If the guard leaked, that count moves.
//
// The reads are checked too, in both directions: a read-only connection that
// cannot run SELECT is not a safety feature, it is a broken connection.
//
// Usage: verify_read_only_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time, and one of MySQL/MariaDB at a time.

use fltk::{app, input::IntInput};
use space_query::db::{ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TABLE: &str = "OQT_READ_ONLY";

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
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1521);
                let service = env::var("ORACLE_TEST_SERVICE_NAME")
                    .or_else(|_| env::var("ORACLE_TEST_SERVICE"))
                    .unwrap_or_else(|_| "FREE".into());
                let user = env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into());
                let pass = env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".into());
                let mut info = ConnectionInfo::new_with_type(
                    "prod",
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
                "prod",
                "root",
                "spacequery",
                "127.0.0.1",
                3307,
                "query_tool_mysql8",
                DatabaseType::MySQL,
            ),
            Target::MariaDb => ConnectionInfo::new_with_type(
                "prod",
                "root",
                "password",
                "127.0.0.1",
                3306,
                "query_tool_test",
                DatabaseType::MariaDB,
            ),
        }
    }

    fn create_sql(self) -> String {
        if self.is_oracle() {
            format!("CREATE TABLE {TABLE} (ID NUMBER PRIMARY KEY, NAME VARCHAR2(30))")
        } else {
            format!("CREATE TABLE {TABLE} (ID INT PRIMARY KEY, NAME VARCHAR(30))")
        }
    }

    /// Statements a read-only connection must refuse.
    ///
    /// Every one of these is run on a *writable* connection first, and has to
    /// succeed there. Without that control group a refusal proves nothing: a
    /// statement the backend would reject anyway looks exactly like a statement
    /// the guard stopped.
    ///
    /// `restores_fixture` marks the ones that change the table itself, so the
    /// control run can put it back before the next one.
    fn writes(self) -> Vec<Write> {
        let mut writes = vec![
            Write::row_change(
                "INSERT",
                format!("INSERT INTO {TABLE} (ID, NAME) VALUES (99, 'NEW')"),
            ),
            Write::row_change("UPDATE", format!("UPDATE {TABLE} SET NAME = 'CHANGED'")),
            Write::row_change("DELETE", format!("DELETE FROM {TABLE}")),
            Write::row_change(
                "a write buried in a script of reads",
                format!("SELECT 1 FROM DUAL; DELETE FROM {TABLE}"),
            ),
            Write::row_change(
                "a write behind a comment that looks like a read",
                format!("/* SELECT */ DELETE FROM {TABLE}"),
            ),
            Write::structural("CREATE TABLE", format!("CREATE TABLE {TABLE}_X (ID INT)")),
            Write::structural("DROP TABLE", format!("DROP TABLE {TABLE}")),
            Write::structural("TRUNCATE", format!("TRUNCATE TABLE {TABLE}")),
            Write::structural(
                "ALTER TABLE",
                format!("ALTER TABLE {TABLE} ADD EXTRA_COL INT"),
            ),
            // Looks like a read and is not: it takes row locks that outlive the
            // statement.
            Write::row_change(
                "SELECT ... FOR UPDATE",
                format!("SELECT * FROM {TABLE} FOR UPDATE"),
            ),
        ];
        if self.is_oracle() {
            writes.extend([
                Write::row_change("a PL/SQL block", format!("BEGIN DELETE FROM {TABLE}; END;")),
                Write::row_change(
                    "a DECLARE block",
                    format!("DECLARE n NUMBER; BEGIN DELETE FROM {TABLE}; END;"),
                ),
                Write::row_change(
                    "MERGE",
                    format!(
                        "MERGE INTO {TABLE} t USING (SELECT 1 AS ID FROM DUAL) s \
                         ON (t.ID = s.ID) WHEN MATCHED THEN UPDATE SET t.NAME = 'MERGED'"
                    ),
                ),
                Write::row_change(
                    "INSERT ALL",
                    format!(
                        "INSERT ALL INTO {TABLE} (ID, NAME) VALUES (97, 'A') \
                         INTO {TABLE} (ID, NAME) VALUES (98, 'B') SELECT 1 FROM DUAL"
                    ),
                ),
                Write::structural(
                    "CREATE OR REPLACE PROCEDURE",
                    format!("CREATE OR REPLACE PROCEDURE {TABLE}_P AS BEGIN NULL; END;"),
                ),
                Write::structural("GRANT", format!("GRANT SELECT ON {TABLE} TO PUBLIC")),
                // Oracle's EXPLAIN PLAN inserts rows into PLAN_TABLE, so it is
                // a write and is refused. MySQL's EXPLAIN only reports, and is
                // in `reads`.
                Write::row_change(
                    "EXPLAIN PLAN (writes to PLAN_TABLE)",
                    format!("EXPLAIN PLAN FOR SELECT * FROM {TABLE}"),
                ),
            ]);
        } else {
            writes.extend([
                Write::row_change(
                    "REPLACE INTO",
                    format!("REPLACE INTO {TABLE} (ID, NAME) VALUES (1, 'REPLACED')"),
                ),
                Write::row_change(
                    "INSERT ... ON DUPLICATE KEY UPDATE",
                    format!(
                        "INSERT INTO {TABLE} (ID, NAME) VALUES (1, 'X') \
                         ON DUPLICATE KEY UPDATE NAME = 'DUP'"
                    ),
                ),
                Write::row_change(
                    "a write inside an executable comment",
                    format!("/*! DELETE FROM {TABLE} */"),
                ),
                Write::structural("RENAME TABLE", format!("RENAME TABLE {TABLE} TO {TABLE}_X")),
                Write::structural(
                    "CREATE PROCEDURE",
                    format!("CREATE PROCEDURE {TABLE}_P() BEGIN SELECT 1; END"),
                ),
            ]);
        }
        // Not run on the writable connection: the include names a file that is
        // not there, and a real CONNECT would swap the connection out from under
        // the harness. Both are refused for what they are, not for what they
        // would do.
        writes.push(Write::unrunnable("a script include", "@somewhere.sql"));
        writes.push(Write::unrunnable(
            "a SQL*Plus CONNECT",
            "CONNECT other/pw@127.0.0.1:1521/FREE",
        ));
        writes
    }

    /// Statements a read-only connection must still run. A connection that
    /// cannot SELECT is not a safety feature.
    fn reads(self) -> Vec<(&'static str, String)> {
        let mut reads = vec![
            (
                "SELECT",
                format!("SELECT ID, NAME FROM {TABLE} ORDER BY ID"),
            ),
            (
                "a WITH query",
                format!("WITH t AS (SELECT ID FROM {TABLE}) SELECT COUNT(*) AS N FROM t"),
            ),
            (
                "several SELECTs at once",
                "SELECT 1 FROM DUAL; SELECT 2 FROM DUAL".to_string(),
            ),
            (
                "a scalar subquery and a join",
                format!(
                    "SELECT a.ID, (SELECT COUNT(*) FROM {TABLE}) AS N \
                     FROM {TABLE} a JOIN {TABLE} b ON a.ID = b.ID"
                ),
            ),
            ("COMMIT", "COMMIT".to_string()),
            ("ROLLBACK", "ROLLBACK".to_string()),
        ];
        if self.is_oracle() {
            reads.push((
                "ALTER SESSION SET CURRENT_SCHEMA",
                format!("ALTER SESSION SET CURRENT_SCHEMA = {}", self.schema()),
            ));
        } else {
            reads.push(("SHOW TABLES", "SHOW TABLES".to_string()));
            reads.push(("USE", format!("USE {}", self.schema())));
            reads.push(("DESCRIBE", format!("DESCRIBE {TABLE}")));
            // MySQL's EXPLAIN only reports; Oracle's writes to PLAN_TABLE and
            // is in `writes` instead.
            reads.push(("EXPLAIN", format!("EXPLAIN SELECT * FROM {TABLE}")));
        }
        reads
    }

    /// A server setting this backend lets a privileged account change, and the
    /// statement that changes it. Read back over the WRITABLE connection, so a
    /// refusal that still reached the server is visible; restored afterwards.
    fn server_setting(self) -> (&'static str, fn(i64) -> String) {
        if self.is_oracle() {
            (
                "SELECT VALUE FROM V$PARAMETER WHERE NAME = 'open_cursors'",
                |value| format!("ALTER SYSTEM SET open_cursors = {value} SCOPE=MEMORY"),
            )
        } else {
            ("SELECT @@GLOBAL.net_read_timeout", |value| {
                format!("SET GLOBAL net_read_timeout = {value}")
            })
        }
    }

    fn schema(self) -> String {
        match self {
            Target::OracleThin | Target::OracleOci => {
                env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into())
            }
            Target::MySql => "query_tool_mysql8".to_string(),
            Target::MariaDb => "query_tool_test".to_string(),
        }
    }

    /// Everything the fixture may have left behind, in drop order.
    fn cleanup_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!("DROP PROCEDURE {TABLE}_P"),
                format!("DROP TABLE {TABLE}_X"),
                format!("DROP TABLE {TABLE}"),
            ]
        } else {
            vec![
                format!("DROP PROCEDURE IF EXISTS {TABLE}_P"),
                format!("DROP TABLE IF EXISTS {TABLE}_X"),
                format!("DROP TABLE IF EXISTS {TABLE}"),
            ]
        }
    }
}

/// A statement the guard has to refuse, and how to prove it is a real write.
struct Write {
    what: &'static str,
    sql: String,
    /// Whether a writable connection is expected to run this successfully.
    ///
    /// False only for the two statements that cannot be demonstrated in this
    /// harness at all — see `Target::writes`.
    runnable: bool,
    /// Whether running it changes the table itself, so the control run has to
    /// rebuild the fixture afterwards.
    structural: bool,
}

impl Write {
    fn row_change(what: &'static str, sql: String) -> Self {
        Self {
            what,
            sql,
            runnable: true,
            structural: false,
        }
    }

    fn structural(what: &'static str, sql: String) -> Self {
        Self {
            what,
            sql,
            runnable: true,
            structural: true,
        }
    }

    fn unrunnable(what: &'static str, sql: &str) -> Self {
        Self {
            what,
            sql: sql.to_string(),
            runnable: false,
            structural: false,
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

struct Harness {
    editor: SqlEditorWidget,
    events: Arc<Mutex<Vec<QueryProgress>>>,
    done: Arc<AtomicBool>,
}

impl Harness {
    fn new(shared: Arc<Mutex<DatabaseConnection>>) -> Self {
        let timeout_input = IntInput::default();
        let mut editor = SqlEditorWidget::new(shared, timeout_input);
        let events = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        {
            let events = Arc::clone(&events);
            let done = Arc::clone(&done);
            editor.set_progress_callback(move |event| {
                if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                    done.store(true, Ordering::SeqCst);
                }
                events
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event);
            });
        }
        Self {
            editor,
            events,
            done,
        }
    }

    /// Run `sql` and report whether the editor started anything at all.
    ///
    /// A refused statement produces no batch, so waiting for one would only
    /// time out; the wait is bounded and its expiry is the answer, not a
    /// failure.
    fn attempt(&mut self, sql: &str, expect_run: bool) -> bool {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let seconds = if expect_run { 60 } else { 2 };
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !self.done.load(Ordering::SeqCst) && Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let drain = Instant::now() + Duration::from_millis(150);
        while Instant::now() < drain {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        !self.events().is_empty()
    }

    fn run(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        if !self.attempt(sql, true) {
            return Err(format!("nothing ran for: {sql}"));
        }
        let events = self.events();
        if let Some(error) = events.iter().find_map(|event| match progress_inner(event) {
            QueryProgress::StatementFinished { result, .. } if !result.success => {
                Some(result.message.clone())
            }
            _ => None,
        }) {
            return Err(error);
        }
        Ok(events)
    }

    fn events(&self) -> Vec<QueryProgress> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn connect(info: ConnectionInfo) -> Result<Arc<Mutex<DatabaseConnection>>, String> {
    let mut connection = DatabaseConnection::new();
    connection
        .connect(info)
        .map_err(|err| format!("connect: {err}"))?;
    Ok(Arc::new(Mutex::new(connection)))
}

/// The row count as a second, writable connection sees it.
/// The first cell of `sql`, as an integer. `row_count`'s twin, for the server
/// settings the reconfiguration check reads back.
fn scalar_i64(writer: &mut Harness, sql: &str) -> Result<i64, String> {
    let events = writer.run(sql)?;
    for event in &events {
        let rows = match progress_inner(event) {
            QueryProgress::Rows { rows, .. } => rows.clone(),
            QueryProgress::StatementFinished { result, .. } => result.rows.clone(),
            _ => continue,
        };
        if let Some(value) = rows.first().and_then(|row| row.last()) {
            return value
                .trim()
                .parse::<i64>()
                .map_err(|err| format!("{sql} returned {value:?}: {err}"));
        }
    }
    Err(format!("{sql} returned nothing"))
}

fn row_count(writer: &mut Harness) -> Result<i64, String> {
    let events = writer.run(&format!("SELECT COUNT(*) AS N FROM {TABLE}"))?;
    for event in &events {
        match progress_inner(event) {
            QueryProgress::Rows { rows, .. } => {
                if let Some(row) = rows.first() {
                    if let Some(value) = row.last() {
                        return value
                            .trim()
                            .parse::<i64>()
                            .map_err(|err| format!("row count {value:?}: {err}"));
                    }
                }
            }
            QueryProgress::StatementFinished { result, .. } => {
                if let Some(row) = result.rows.first() {
                    if let Some(value) = row.last() {
                        return value
                            .trim()
                            .parse::<i64>()
                            .map_err(|err| format!("row count {value:?}: {err}"));
                    }
                }
            }
            _ => {}
        }
    }
    Err("the row count query returned nothing".to_string())
}

/// Build the fixture from scratch over a writable connection.
fn build_fixture(writer: &mut Harness, target: Target) -> Result<(), String> {
    let _ = writer.run("COMMIT");
    for sql in target.cleanup_sql() {
        let _ = writer.run(&sql);
    }
    writer
        .run(&target.create_sql())
        .map_err(|err| format!("create: {err}"))?;
    for id in 1..=3 {
        writer
            .run(&format!(
                "INSERT INTO {TABLE} (ID, NAME) VALUES ({id}, 'ROW{id}')"
            ))
            .map_err(|err| format!("insert {id}: {err}"))?;
    }
    writer.run("COMMIT")?;
    Ok(())
}

/// Prove every refusal candidate is a statement this backend really accepts.
///
/// Without this the guard could be "protecting" against SQL the server would
/// have thrown out anyway, and a typo would read as a pass.
fn control_group(writer: &mut Harness, target: Target) -> Result<(), String> {
    println!("-- control: the same statements on a writable connection --");
    for write in target.writes() {
        if !write.runnable {
            println!("SKIP: {} is not demonstrable in this harness", write.what);
            continue;
        }
        writer.run(&write.sql).map_err(|err| {
            format!(
                "control: a writable connection could not run {} ({}): {err}",
                write.what, write.sql
            )
        })?;
        let _ = writer.run("COMMIT");
        println!(
            "PASS: {} really is a write this backend accepts",
            write.what
        );
        if write.structural {
            build_fixture(writer, target)?;
        }
    }
    build_fixture(writer, target)
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());

    // A writable connection builds the fixture and is, afterwards, the
    // independent witness for whether anything actually changed.
    let mut writer = Harness::new(connect(target.connection_info())?);
    build_fixture(&mut writer, target)?;
    control_group(&mut writer, target)?;

    let baseline = row_count(&mut writer)?;
    println!("\n-- guarded: the same statements on a read-only connection --");
    println!("(baseline: {baseline} rows)");

    // The connection under test, marked read-only exactly as the connection
    // dialog would mark it.
    let mut read_only_info = target.connection_info();
    read_only_info.read_only = true;
    let mut guarded = Harness::new(connect(read_only_info)?);

    for write in target.writes() {
        if guarded.attempt(&write.sql, false) {
            return Err(format!("{} was allowed to run: {}", write.what, write.sql));
        }
        let after = row_count(&mut writer)?;
        if after != baseline {
            return Err(format!(
                "{} changed the database: {baseline} rows became {after}",
                write.what
            ));
        }
        println!(
            "PASS: {} was refused and the database is unchanged",
            write.what
        );
    }

    // The structural attempts must not have created, renamed or dropped
    // anything either — a row count cannot see that.
    writer
        .run(&format!("SELECT COUNT(*) AS N FROM {TABLE}"))
        .map_err(|err| {
            format!("the table the guard was told not to drop or rename is gone: {err}")
        })?;
    if writer
        .run(&format!("SELECT COUNT(*) AS N FROM {TABLE}_X"))
        .is_ok()
    {
        return Err("the refused CREATE TABLE or RENAME was executed after all".into());
    }
    if writer
        .run(&format!("SELECT EXTRA_COL FROM {TABLE}"))
        .is_ok()
    {
        return Err("the refused ALTER TABLE was executed after all".into());
    }
    println!("PASS: nothing was created, renamed, altered or dropped");

    // A statement that reconfigures the SERVER is the one a `SqlKind` cannot
    // answer about: Oracle's `ALTER SYSTEM` is session control (it carries no
    // implicit commit) and `SET GLOBAL TRANSACTION ...` is transaction control,
    // and neither answer says whether the effect leaves the session. The guard
    // used to read the kind alone and let them through — on a connection the
    // user had marked read-only, and while the app's OTHER read-only guard, a
    // tab's READ ONLY pin, refused the first of them.
    //
    // A row count cannot see this, so the setting itself is read back over the
    // writable connection, and the control run afterwards is what makes the
    // refusal proof of the guard rather than of a privilege the account lacks.
    println!("\n-- a statement that reconfigures the server --");
    let (read_setting, set_setting) = target.server_setting();
    let before_setting = scalar_i64(&mut writer, read_setting)?;
    if guarded.attempt(&set_setting(before_setting + 1), false) {
        return Err(format!(
            "a read-only connection was allowed to run {}",
            set_setting(before_setting + 1)
        ));
    }
    let during_setting = scalar_i64(&mut writer, read_setting)?;
    if during_setting != before_setting {
        return Err(format!(
            "the refused statement reached the server after all: {read_setting} went from \
             {before_setting} to {during_setting}"
        ));
    }
    println!("PASS: the server reconfiguration was refused and the setting is unchanged");
    writer
        .run(&set_setting(before_setting + 1))
        .map_err(|err| format!("the writable connection could not change the setting: {err}"))?;
    let control_setting = scalar_i64(&mut writer, read_setting)?;
    let _ = writer.run(&set_setting(before_setting));
    if control_setting != before_setting + 1 {
        return Err(format!(
            "the control run did not change the setting either, so the refusal proves nothing: \
             {read_setting} = {control_setting}"
        ));
    }
    if scalar_i64(&mut writer, read_setting)? != before_setting {
        return Err("the server setting was not restored".into());
    }
    println!("PASS: the same statement runs on a writable connection, so the guard refused it");

    println!("\n-- reads must still run on the read-only connection --");
    for (what, sql) in target.reads() {
        guarded
            .run(&sql)
            .map_err(|err| format!("a read-only connection refused {what}: {err}"))?;
        println!("PASS: {what} still runs on a read-only connection");
    }

    // And the guard is the *connection's* property, not a global mode: the
    // writable connection in this same process is still writable.
    writer
        .run(&format!(
            "INSERT INTO {TABLE} (ID, NAME) VALUES (42, 'STILL WRITABLE')"
        ))
        .map_err(|err| format!("the writable connection stopped writing: {err}"))?;
    writer.run("COMMIT")?;
    if row_count(&mut writer)? != baseline + 1 {
        return Err("the writable connection's INSERT did not land".into());
    }
    println!("PASS: a writable connection in the same process is unaffected");

    for sql in target.cleanup_sql() {
        let _ = writer.run(&sql);
    }
    let _ = writer.run("COMMIT");
    Ok(())
}

fn main() {
    let _app = app::App::default();
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

    let mut failures: Vec<String> = Vec::new();
    for target in targets {
        match verify(target) {
            Ok(()) => println!("\n{} OK", target.label()),
            Err(err) => {
                eprintln!("\n{} FAILED: {err}", target.label());
                failures.push(format!("{}: {err}", target.label()));
            }
        }
    }

    println!();
    if failures.is_empty() {
        println!("ALL CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}
