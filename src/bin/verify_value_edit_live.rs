#![allow(clippy::cargo, clippy::pedantic)]

// Live verification that a long value edited in the grid actually reaches the
// database intact, on every supported backend: Oracle Thin, Oracle OCI, MySQL
// and MariaDB.
//
// This is the half of the value window (item 4) that a unit test cannot reach.
// The window makes a CLOB editable; whether the edit *saves* was, until now,
// where Oracle stopped: the grid's save path renders values as SQL string
// literals, and a literal over 4000 bytes is ORA-01704. So a CLOB cell could be
// read in full and never written back.
//
// `oracle_text_literal` now chunks a long value into `TO_CLOB(..) || TO_CLOB(..)`.
// Only a server can say whether the chunking is right, and it has to be right
// in two places, because that is what `save_edit_mode` emits for an edited row:
//
//     UPDATE t SET c = <new value> WHERE ROWID = '..' AND c = <original value>
//
// Both sides are user text and both can be long. The checks below drive that
// exact statement shape, with a value that has multi-byte characters, embedded
// single quotes and newlines, then read the value back and compare it byte for
// byte.
//
// MySQL and MariaDB take the structured (bind) path instead of literals, so the
// same value goes through `ResultEditRequest` — the production request the grid
// builds — to prove the round trip there too.
//
// Usage: verify_value_edit_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time, and one of MySQL/MariaDB at a time.

use fltk::{app, input::IntInput};
use space_query::db::{
    compile_oracle_guarded_result_edit, ConnectionInfo, DatabaseConnection, DatabaseType,
    OracleDriverMode, ResultEditAssignment, ResultEditColumn, ResultEditDescriptor,
    ResultEditMutation, ResultEditOriginalValue, ResultEditRequest, ResultEditScalar,
    ResultEditValue,
};
use space_query::ui::result_table::ResultTableWidget;
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TABLE: &str = "OQT_LONG_VALUE";

/// A value far past Oracle's 4000-byte literal limit, built from a unit that
/// exercises every way the chunker can go wrong at once: three-byte characters
/// (a cut on a byte boundary would panic or corrupt), single quotes (which
/// double on escape, so a cut in the wrong place splits an escaped pair) and
/// newlines.
fn long_value() -> String {
    "값 'quote' 줄\n".repeat(700)
}

/// A second long value, so the update has a different original to compare
/// against and cannot pass by writing nothing.
fn other_long_value() -> String {
    "다른 'value' 행\n".repeat(800)
}

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

    fn create_sql(self) -> String {
        if self.is_oracle() {
            format!("CREATE TABLE {TABLE} (ID NUMBER PRIMARY KEY, BODY CLOB)")
        } else {
            format!("CREATE TABLE {TABLE} (ID INT PRIMARY KEY, BODY LONGTEXT)")
        }
    }

    fn drop_sql(self) -> String {
        if self.is_oracle() {
            format!("DROP TABLE {TABLE}")
        } else {
            format!("DROP TABLE IF EXISTS {TABLE}")
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
    fn pump_until<F: Fn() -> bool>(&self, label: &str, pred: F) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(60);
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
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("statement to finish", || done.load(Ordering::SeqCst))?;
        let events = self.events();
        if let Some(error) = first_error(&events) {
            return Err(error);
        }
        Ok(events)
    }

    fn run_edit(&mut self, request: ResultEditRequest) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_result_edit(request)?;
        let done = Arc::clone(&self.done);
        self.pump_until("result edit to finish", || done.load(Ordering::SeqCst))?;
        let events = self.events();
        if let Some(error) = first_error(&events) {
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

fn first_error(events: &[QueryProgress]) -> Option<String> {
    events.iter().find_map(|event| match progress_inner(event) {
        QueryProgress::StatementFinished { result, .. } if !result.success => {
            Some(result.message.clone())
        }
        _ => None,
    })
}

/// Read the first data row's value for the named column.
///
/// The editor prepends a ROWID column to Oracle SELECTs, so reading by name
/// rather than position is required.
fn cell_by_col(events: &[QueryProgress], col_name: &str) -> Option<String> {
    let mut index = None;
    for event in events {
        if let QueryProgress::SelectStart { columns, .. } = progress_inner(event) {
            index = columns
                .iter()
                .position(|column| column.trim_matches('"').eq_ignore_ascii_case(col_name));
        }
    }
    let index = index?;
    for event in events {
        match progress_inner(event) {
            QueryProgress::Rows { rows, .. } => {
                if let Some(first) = rows.first() {
                    return first.get(index).cloned();
                }
            }
            QueryProgress::StatementFinished { result, .. } => {
                if let Some(first) = result.rows.first() {
                    return first.get(index).cloned();
                }
            }
            _ => {}
        }
    }
    None
}

fn oracle_rowid(harness: &mut Harness, id: u32) -> Result<String, String> {
    let events = harness.run(&format!(
        "SELECT ROWIDTOCHAR(ROWID) AS RID FROM {TABLE} WHERE ID = {id}"
    ))?;
    cell_by_col(&events, "RID")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("row {id} did not return a usable ROWID"))
}

/// Report the first difference between what was written and what came back.
fn compare(expected: &str, actual: &str, what: &str) -> Result<(), String> {
    if expected == actual {
        return Ok(());
    }
    let shared = expected
        .bytes()
        .zip(actual.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    Err(format!(
        "{what}: value differs after {shared} identical bytes \
         (wrote {} bytes, read back {} bytes); \
         wrote {:?}..., read {:?}...",
        expected.len(),
        actual.len(),
        &expected[shared..expected.len().min(shared + 40)],
        &actual[shared..actual.len().min(shared + 40)],
    ))
}

/// The SET/WHERE statement `save_edit_mode` emits for an edited existing row of
/// a CLOB column, built from the same literal helper.
///
/// The `DBMS_LOB.COMPARE` guard is not decoration: `BODY = 'seed'` on a CLOB is
/// ORA-22848 no matter how short the value is, which is why a table with a CLOB
/// column could not be edited at all before this.
fn oracle_guarded_update(
    rowid: &str,
    new_value: &str,
    original_value: &str,
) -> Result<String, String> {
    let new_literal = ResultTableWidget::sql_literal_from_input_with_null_text(new_value, "NULL")?;
    let original_literal =
        ResultTableWidget::sql_literal_from_input_with_null_text(original_value, "NULL")?;
    compile_oracle_guarded_result_edit(&[format!(
        "UPDATE {TABLE} SET BODY = {new_literal} \
         WHERE ROWID = '{}' AND DBMS_LOB.COMPARE(BODY, {original_literal}) = 0",
        rowid.replace('\'', "''")
    )])
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());

    let mut connection = DatabaseConnection::new();
    connection
        .connect(target.connection_info())
        .map_err(|err| format!("connect: {err}"))?;
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
            events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });
    }
    let mut harness = Harness {
        editor,
        events,
        done,
    };

    let first = long_value();
    let second = other_long_value();
    println!(
        "(value sizes: {} bytes and {} bytes; Oracle's literal limit is 4000)",
        first.len(),
        second.len()
    );

    let _ = harness.run("COMMIT");
    let _ = harness.run(&target.drop_sql());
    harness
        .run(&target.create_sql())
        .map_err(|err| format!("create: {err}"))?;
    harness
        .run(&format!(
            "INSERT INTO {TABLE} (ID, BODY) VALUES (1, 'seed')"
        ))
        .map_err(|err| format!("insert: {err}"))?;
    harness
        .run("COMMIT")
        .map_err(|err| format!("commit baseline: {err}"))?;

    // (1) Write a long value where the original is short. On Oracle this is the
    //     SET side of the chunker; on the MySQL family it is the bind path.
    if target.is_oracle() {
        let rowid = oracle_rowid(&mut harness, 1)?;
        let sql = oracle_guarded_update(&rowid, &first, "seed")?;
        if !sql.contains("TO_CLOB(") {
            return Err("the long value was not chunked, so this proves nothing".into());
        }
        harness
            .run(&sql)
            .map_err(|err| format!("long-value UPDATE: {err}"))?;
    } else {
        harness
            .run_edit(structured_update(target, &first, "seed"))
            .map_err(|err| format!("long-value structured edit: {err}"))?;
    }
    harness.run("COMMIT")?;

    let read_back = harness.run(&format!("SELECT BODY FROM {TABLE} WHERE ID = 1"))?;
    let stored = cell_by_col(&read_back, "BODY").unwrap_or_default();
    compare(&first, &stored, "(1) long value written over a short one")?;
    println!(
        "PASS(1): {} bytes written and read back unchanged",
        first.len()
    );

    // (2) Now replace it with a different long value, comparing against the
    //     long original. On Oracle this is the WHERE side of the chunker — the
    //     half that a SET-only test would miss.
    if target.is_oracle() {
        let rowid = oracle_rowid(&mut harness, 1)?;
        let sql = oracle_guarded_update(&rowid, &second, &first)?;
        // The guarded block raises ORA-20001 unless exactly one row matched, so
        // a long original that failed to compare equal surfaces as an error
        // here rather than as a silent no-op.
        harness.run(&sql).map_err(|err| {
            format!("long-original UPDATE (the long original did not compare equal?): {err}")
        })?;
    } else {
        harness
            .run_edit(structured_update(target, &second, &first))
            .map_err(|err| format!("long-original structured edit: {err}"))?;
    }
    harness.run("COMMIT")?;

    let read_back = harness.run(&format!("SELECT BODY FROM {TABLE} WHERE ID = 1"))?;
    let stored = cell_by_col(&read_back, "BODY").unwrap_or_default();
    compare(&second, &stored, "(2) long value written over a long one")?;
    println!(
        "PASS(2): a {}-byte original compared equal and was replaced by {} bytes",
        first.len(),
        second.len()
    );

    // (3) A short value still saves the way it always did.
    if target.is_oracle() {
        let rowid = oracle_rowid(&mut harness, 1)?;
        let sql = oracle_guarded_update(&rowid, "back to short", &second)?;
        harness
            .run(&sql)
            .map_err(|err| format!("short-value UPDATE: {err}"))?;
    } else {
        harness
            .run_edit(structured_update(target, "back to short", &second))
            .map_err(|err| format!("short-value structured edit: {err}"))?;
    }
    harness.run("COMMIT")?;
    let read_back = harness.run(&format!("SELECT BODY FROM {TABLE} WHERE ID = 1"))?;
    let stored = cell_by_col(&read_back, "BODY").unwrap_or_default();
    compare("back to short", &stored, "(3) short value")?;
    println!("PASS(3): a short value still round-trips");

    let _ = harness.run(&target.drop_sql());
    let _ = harness.run("COMMIT");
    Ok(())
}

/// The request the MySQL-family grid builds when one cell of one row changed.
fn structured_update(target: Target, new_value: &str, original: &str) -> ResultEditRequest {
    let info = target.connection_info();
    ResultEditRequest {
        request_id: 1,
        descriptor: ResultEditDescriptor {
            db_type: info.db_type,
            schema_name: info.service_name,
            table_name: TABLE.to_string(),
            locator_columns: vec!["ID".to_string()],
            editable_columns: vec![
                ResultEditColumn {
                    result_index: 0,
                    source_name: "ID".to_string(),
                },
                ResultEditColumn {
                    result_index: 1,
                    source_name: "BODY".to_string(),
                },
            ],
            snapshot_column_index: 2,
        },
        mutations: vec![ResultEditMutation::Update {
            locator_values: vec![ResultEditScalar::Int(1)],
            original_values: vec![ResultEditOriginalValue {
                column_name: "BODY".to_string(),
                value: ResultEditScalar::text(original),
            }],
            assignments: vec![ResultEditAssignment {
                column_name: "BODY".to_string(),
                value: ResultEditValue::Text(new_value.to_string()),
            }],
        }],
    }
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
