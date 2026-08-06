#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the result-grid SQL export menu items — SQL Inserts,
// SQL Updates, Where Clause — across every supported backend: Oracle Thin,
// Oracle OCI, MySQL, and MariaDB.
//
// This is the only check that can prove the two things unit tests cannot:
//   (1) each driver classifies real server column types into the right
//       `SqlValueKind`, so literals are quoted (or not) correctly, and
//   (2) the generated SQL actually executes and round-trips the values.
//
// It drives the real `SqlEditorWidget` execution worker (the same plumbing the
// GUI uses), takes the column kinds straight off the real
// `QueryProgress::SelectStart` event, generates SQL with the production
// builders, then executes that SQL against the server and compares the result
// with the source rows.
//
// The WHERE clause is proven by counting: the generated condition must match
// exactly the rows that were selected, no more and no fewer.
//
// Usage: verify_grid_sql_export_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Docker container at a time.

use fltk::{app, input::IntInput};
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode, SqlValueKind,
};
use space_query::ui::grid_sql_export::{
    build_sql_inserts, build_sql_updates, build_where_clause, resolve_export_table,
    GridSqlSelection,
};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use space_query::ui::ObjectBrowserWidget;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Column names, their driver-classified kinds, and the row values — the three
/// things the grid receives for a result set.
type GridView = (Vec<String>, Vec<SqlValueKind>, Vec<Vec<String>>);

const BASE_TABLE: &str = "OQT_SQL_EXPORT_SRC";
const COPY_TABLE: &str = "OQT_SQL_EXPORT_DST";

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

    /// A column per `SqlValueKind` the backend can express, plus a composite
    /// primary key so `SQL Updates` has a real multi-column key to find.
    fn create_sql(self, table: &str) -> String {
        if self.is_oracle() {
            format!(
                "CREATE TABLE {table} (\
                 PART VARCHAR2(10) NOT NULL, \
                 SEQ NUMBER NOT NULL, \
                 NAME VARCHAR2(50), \
                 CODE VARCHAR2(10), \
                 AMT NUMBER(12,2), \
                 HIRED DATE, \
                 TS TIMESTAMP(6), \
                 TSZ TIMESTAMP(6) WITH TIME ZONE, \
                 RAWC RAW(8), \
                 CONSTRAINT {table}_PK PRIMARY KEY (PART, SEQ))"
            )
        } else {
            format!(
                "CREATE TABLE {table} (\
                 PART VARCHAR(10) NOT NULL, \
                 SEQ INT NOT NULL, \
                 NAME VARCHAR(50), \
                 CODE VARCHAR(10), \
                 AMT DECIMAL(12,2), \
                 HIRED DATE, \
                 TS DATETIME(6), \
                 FLAG TINYINT, \
                 BLOBC BLOB, \
                 PRIMARY KEY (PART, SEQ))"
            )
        }
    }

    fn drop_sql(self, table: &str) -> String {
        if self.is_oracle() {
            format!("DROP TABLE {table}")
        } else {
            format!("DROP TABLE IF EXISTS {table}")
        }
    }

    /// Two rows: one fully populated (including a quote in the text and a
    /// zero-padded code that must not turn into a number), one mostly NULL.
    fn seed_sql(self, table: &str) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!(
                    "INSERT INTO {table} (PART, SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     'A-1', 1, 'it''s SMITH', '00123', 1234.56, \
                     TO_DATE('1980-12-17 09:30:00','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('1980-12-17 09:30:00.123456','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('1980-12-17 09:30:00.123456 +09:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     HEXTORAW('DEADBEEF'))"
                ),
                format!(
                    "INSERT INTO {table} (PART, SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     'B-2', 2, NULL, '007', NULL, NULL, NULL, NULL, NULL)"
                ),
            ]
        } else {
            vec![
                format!(
                    "INSERT INTO {table} (PART, SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BLOBC) VALUES (\
                     'A-1', 1, 'it''s SMITH', '00123', 1234.56, '1980-12-17', \
                     '1980-12-17 09:30:00.123456', 1, NULL)"
                ),
                format!(
                    "INSERT INTO {table} (PART, SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BLOBC) VALUES (\
                     'B-2', 2, NULL, '007', NULL, NULL, NULL, NULL, NULL)"
                ),
            ]
        }
    }

    fn select_sql(self, table: &str) -> String {
        format!("SELECT * FROM {table} ORDER BY PART, SEQ")
    }

    /// Kinds every backend must report for the fixture columns that matter.
    fn expected_kinds(self) -> Vec<(&'static str, SqlValueKind)> {
        let mut expected = vec![
            ("PART", SqlValueKind::String),
            ("SEQ", SqlValueKind::Number),
            ("NAME", SqlValueKind::String),
            ("CODE", SqlValueKind::String),
            ("AMT", SqlValueKind::Number),
            ("HIRED", SqlValueKind::Temporal),
            ("TS", SqlValueKind::Temporal),
        ];
        if self.is_oracle() {
            expected.push(("TSZ", SqlValueKind::Temporal));
            expected.push(("RAWC", SqlValueKind::Binary));
        } else {
            expected.push(("FLAG", SqlValueKind::Number));
            // BINARY/VARBINARY reach the client as VAR_STRING with a binary
            // charset, so BLOB is the only MySQL type that classifies as Binary.
            expected.push(("BLOBC", SqlValueKind::Binary));
        }
        expected
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
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("statement to finish", || done.load(Ordering::SeqCst))?;
        let events = self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(error) = first_error(&events) {
            return Err(error);
        }
        Ok(events)
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

/// The statement text the first `SelectStart` carried.
///
/// The grid names the export table from this while the statement is unfinished,
/// which is the only thing a cancelled lazy fetch ever leaves behind.
fn select_start_sql(events: &[QueryProgress]) -> Option<String> {
    events.iter().find_map(|event| match progress_inner(event) {
        QueryProgress::SelectStart { columns, sql, .. } if !columns.is_empty() => Some(sql.clone()),
        _ => None,
    })
}

/// Columns, kinds, and rows exactly as the grid would receive them.
///
/// Editable results carry extra columns the grid never exports: Oracle injects
/// `ROWID`, MySQL and MariaDB append the grid-edit snapshot. Dropping them here
/// mirrors `ResultTableWidget::is_internal_export_column`, so the live check
/// stays honest about what actually reaches the clipboard.
fn grid_view(events: &[QueryProgress]) -> Option<GridView> {
    let (columns, kinds) = events
        .iter()
        .find_map(|event| match progress_inner(event) {
            QueryProgress::SelectStart {
                columns,
                column_kinds,
                ..
            } if !columns.is_empty() => Some((columns.clone(), column_kinds.clone())),
            _ => None,
        })?;

    let mut rows: Vec<Vec<String>> = Vec::new();
    for event in events {
        if let QueryProgress::Rows { rows: batch, .. } = progress_inner(event) {
            rows.extend(batch.clone());
        }
    }

    let keep: Vec<usize> = (0..columns.len())
        .filter(|index| {
            let name = columns[*index].trim_matches('"').to_ascii_uppercase();
            name != "ROWID"
                && !name.ends_with(".ROWID")
                && name != "SQ_INTERNAL_ROWID"
                && name != "SQ_INTERNAL_EDIT_SNAPSHOT"
                && !name.starts_with("SQ_INTERNAL_EDIT_KEY_")
        })
        .collect();

    let columns = keep.iter().map(|i| columns[*i].clone()).collect::<Vec<_>>();
    let kinds = keep
        .iter()
        .map(|i| kinds.get(*i).copied().unwrap_or(SqlValueKind::Unknown))
        .collect::<Vec<_>>();
    let rows = rows
        .into_iter()
        .map(|row| {
            keep.iter()
                .map(|i| row.get(*i).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    Some((columns, kinds, rows))
}

fn selection(
    db_type: DatabaseType,
    table: &str,
    columns: &[String],
    kinds: &[SqlValueKind],
    rows: &[Vec<String>],
    selected_columns: Vec<usize>,
    selected_rows: Vec<usize>,
) -> GridSqlSelection {
    GridSqlSelection {
        db_type,
        table: Some(table.to_string()),
        all_columns: columns.to_vec(),
        column_kinds: kinds.to_vec(),
        selected_columns,
        rows: selected_rows
            .into_iter()
            .filter_map(|index| rows.get(index).cloned())
            .collect(),
        null_text: "NULL".to_string(),
    }
}

fn column_index(columns: &[String], name: &str) -> Result<usize, String> {
    columns
        .iter()
        .position(|column| column.trim_matches('"').eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("column {name} missing from the result set"))
}

fn single_count(events: &[QueryProgress]) -> Option<i64> {
    for event in events {
        if let QueryProgress::Rows { rows, .. } = progress_inner(event) {
            if let Some(value) = rows.first().and_then(|row| row.last()) {
                return value.trim().parse().ok();
            }
        }
    }
    None
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());
    let info = target.connection_info();
    let db_type = info.db_type;

    let mut connection = DatabaseConnection::new();
    connection
        .connect(info)
        .map_err(|e| format!("connect: {e}"))?;
    let shared: Arc<Mutex<DatabaseConnection>> = Arc::new(Mutex::new(connection));

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
            events.lock().unwrap_or_else(|p| p.into_inner()).push(event);
        });
    }
    let mut h = Harness {
        editor,
        events,
        done,
    };

    // Fixtures.
    let _ = h.run(&target.drop_sql(COPY_TABLE));
    let _ = h.run(&target.drop_sql(BASE_TABLE));
    h.run(&target.create_sql(BASE_TABLE))
        .map_err(|e| format!("create source: {e}"))?;
    h.run(&target.create_sql(COPY_TABLE))
        .map_err(|e| format!("create copy: {e}"))?;
    for sql in target.seed_sql(BASE_TABLE) {
        h.run(&sql).map_err(|e| format!("seed: {e}"))?;
    }
    let _ = h.run("COMMIT");

    // The grid's view of the source rows, including the driver's column kinds.
    let select_events = h
        .run(&target.select_sql(BASE_TABLE))
        .map_err(|e| format!("select source: {e}"))?;
    let (columns, kinds, rows) =
        grid_view(&select_events).ok_or_else(|| "SELECT produced no grid columns".to_string())?;
    println!("columns and driver-classified kinds:");
    for (column, kind) in columns.iter().zip(kinds.iter()) {
        println!("  {column:<6} {kind:?}");
    }
    println!("rows: {rows:?}");
    if rows.len() != 2 {
        return Err(format!("expected 2 source rows, got {}", rows.len()));
    }

    // (1) Every fixture column must be classified as the right kind. This is
    //     what decides quoting, so a wrong kind here is a wrong INSERT.
    for (name, expected) in target.expected_kinds() {
        let index = column_index(&columns, name)?;
        let actual = kinds.get(index).copied().unwrap_or(SqlValueKind::Unknown);
        if actual != expected {
            return Err(format!(
                "column {name} classified as {actual:?}, expected {expected:?}"
            ));
        }
    }
    println!("PASS: all fixture columns classified as expected");

    // (1b) The base table must be resolvable from what `SelectStart` carried,
    //      not only from the finished result: a lazy fetch that is still
    //      running — or that the user cancelled — never sends one, and the
    //      export would fall back to the MY_TABLE placeholder.
    let start_sql = select_start_sql(&select_events)
        .ok_or_else(|| "SELECT produced no SelectStart statement text".to_string())?;
    let resolved = resolve_export_table(None, &start_sql);
    let names_base_table = resolved.as_deref().is_some_and(|table| {
        table
            .rsplit('.')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(BASE_TABLE))
    });
    if !names_base_table {
        return Err(format!(
            "SelectStart SQL {start_sql:?} resolved to {resolved:?}, expected {BASE_TABLE}"
        ));
    }
    println!("PASS: unfinished-statement export resolves the table as {resolved:?}");

    // (2) SQL Inserts must reproduce the rows verbatim in another table.
    let all_columns: Vec<usize> = (0..columns.len()).collect();
    let insert_selection = selection(
        db_type,
        COPY_TABLE,
        &columns,
        &kinds,
        &rows,
        all_columns.clone(),
        vec![0, 1],
    );
    let inserts = build_sql_inserts(&insert_selection);
    println!("--- generated SQL Inserts ---\n{inserts}");
    h.run(&inserts)
        .map_err(|e| format!("generated INSERT statements failed: {e}"))?;
    let _ = h.run("COMMIT");

    let copy_events = h
        .run(&target.select_sql(COPY_TABLE))
        .map_err(|e| format!("select copy: {e}"))?;
    let (_, _, copy_rows) =
        grid_view(&copy_events).ok_or_else(|| "copy SELECT produced no columns".to_string())?;
    if copy_rows != rows {
        return Err(format!(
            "SQL Inserts did not round-trip\n  source: {rows:?}\n  copy:   {copy_rows:?}"
        ));
    }
    println!("PASS: SQL Inserts round-tripped every value exactly");

    // (3) SQL Updates must find the real composite primary key and hit exactly
    //     the rows it names.
    let keys = ObjectBrowserWidget::load_primary_key_columns(&shared, None, BASE_TABLE)
        .map_err(|e| format!("primary key lookup: {e}"))?;
    println!("primary key columns: {keys:?}");
    let key_names: Vec<String> = keys.iter().map(|k| k.to_ascii_uppercase()).collect();
    if key_names != vec!["PART".to_string(), "SEQ".to_string()] {
        return Err(format!("expected composite key [PART, SEQ], got {keys:?}"));
    }

    let name_index = column_index(&columns, "NAME")?;
    let mut updated_rows = rows.clone();
    updated_rows[0][name_index] = "UPDATED'X".to_string();
    updated_rows[1][name_index] = "second".to_string();
    let update_selection = selection(
        db_type,
        COPY_TABLE,
        &columns,
        &kinds,
        &updated_rows,
        vec![name_index],
        vec![0, 1],
    );
    let updates = build_sql_updates(&update_selection, &keys);
    println!("--- generated SQL Updates ---\n{updates}");
    h.run(&updates)
        .map_err(|e| format!("generated UPDATE statements failed: {e}"))?;
    let _ = h.run("COMMIT");

    let updated_events = h
        .run(&target.select_sql(COPY_TABLE))
        .map_err(|e| format!("select copy after update: {e}"))?;
    let (_, _, after_update) = grid_view(&updated_events)
        .ok_or_else(|| "post-update SELECT produced no columns".to_string())?;
    if after_update != updated_rows {
        return Err(format!(
            "SQL Updates did not apply as generated\n  expected: {updated_rows:?}\n  actual:   {after_update:?}"
        ));
    }
    println!("PASS: SQL Updates matched rows by primary key and applied exactly");

    // (4) Where Clause must select exactly the rows it was built from.
    let part_index = column_index(&columns, "PART")?;
    let seq_index = column_index(&columns, "SEQ")?;

    let one_column = selection(
        db_type,
        BASE_TABLE,
        &columns,
        &kinds,
        &rows,
        vec![part_index],
        vec![0, 1],
    );
    let in_clause = build_where_clause(&one_column);
    println!("--- generated Where Clause (one column) ---\n{in_clause}");
    let count_events = h
        .run(&format!(
            "SELECT COUNT(*) AS N FROM {BASE_TABLE} WHERE {in_clause}"
        ))
        .map_err(|e| format!("one-column WHERE clause failed: {e}"))?;
    if single_count(&count_events) != Some(2) {
        return Err(format!(
            "one-column WHERE clause matched {:?} rows, expected 2",
            single_count(&count_events)
        ));
    }

    let key_pair = selection(
        db_type,
        BASE_TABLE,
        &columns,
        &kinds,
        &rows,
        vec![part_index, seq_index],
        vec![0],
    );
    let and_clause = build_where_clause(&key_pair);
    println!("--- generated Where Clause (key of one row) ---\n{and_clause}");
    let count_events = h
        .run(&format!(
            "SELECT COUNT(*) AS N FROM {BASE_TABLE} WHERE {and_clause}"
        ))
        .map_err(|e| format!("multi-column WHERE clause failed: {e}"))?;
    if single_count(&count_events) != Some(1) {
        return Err(format!(
            "multi-column WHERE clause matched {:?} rows, expected 1",
            single_count(&count_events)
        ));
    }

    // A NULL-bearing column: the clause must still match both rows, which is
    // only true because NULL is lifted out of the IN list.
    let null_column = selection(
        db_type,
        BASE_TABLE,
        &columns,
        &kinds,
        &rows,
        vec![name_index],
        vec![0, 1],
    );
    let null_clause = build_where_clause(&null_column);
    println!("--- generated Where Clause (column containing NULL) ---\n{null_clause}");
    let count_events = h
        .run(&format!(
            "SELECT COUNT(*) AS N FROM {BASE_TABLE} WHERE {null_clause}"
        ))
        .map_err(|e| format!("NULL-bearing WHERE clause failed: {e}"))?;
    if single_count(&count_events) != Some(2) {
        return Err(format!(
            "NULL-bearing WHERE clause matched {:?} rows, expected 2",
            single_count(&count_events)
        ));
    }
    println!("PASS: Where Clause matched exactly the selected rows in all three shapes");

    let _ = h.run(&target.drop_sql(COPY_TABLE));
    let _ = h.run(&target.drop_sql(BASE_TABLE));
    let _ = h.run("COMMIT");
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
