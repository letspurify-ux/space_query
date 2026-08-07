#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for file → table import, across every supported backend:
// Oracle Thin, Oracle OCI, MySQL, and MariaDB.
//
// The unit tests in `src/ui/result_import.rs` and `src/ui/table_import.rs`
// prove the parsers and the SQL builder against text the same process wrote.
// They cannot prove the two things that only a server can settle:
//
//   (1) that a column's *declared* type — what
//       `column_kind_for_data_type` reads out of the catalog — produces the
//       same literal as the type the driver reports for the same column, and
//   (2) that the generated INSERT script actually runs and puts the original
//       values back, byte for byte, in every one of the seven formats.
//
// So this probe does the whole loop against a real database, once per format:
//
//     SELECT source  →  export file  →  parse file  →  INSERT script
//                    →  execute  →  SELECT copy  →  compare with source
//
// The file really is written to disk, byte-order mark included, and read back
// from disk, so what the parser sees is what a user's file would carry. The
// script runs through the real `SqlEditorWidget` script executor — the same
// plumbing F5 uses — with a small batch size, so the multi-statement path is
// exercised rather than a single lucky statement.
//
// Usage: verify_import_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time.

use fltk::{app, input::IntInput};
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode, SqlValueKind,
};
use space_query::ui::grid_sql_export::{
    build_sql_inserts, sql_literal_for_value, GridSqlSelection,
};
use space_query::ui::result_export::{render, ExportFormat, ExportGrid};
use space_query::ui::result_import::{detect_format, parse, ImportCell, ImportOptions};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use space_query::ui::table_import::{
    build_insert_script, column_kind_for_data_type, default_mapping, describe, ImportRequest,
    TargetColumn,
};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SOURCE_TABLE: &str = "OQT_IMPORT_SRC";
const COPY_TABLE: &str = "OQT_IMPORT_DST";
const NULL_TEXT: &str = "NULL";
/// Small on purpose: the script must be several statements, not one.
const BATCH_ROWS: usize = 2;

type GridView = (Vec<String>, Vec<SqlValueKind>, Vec<Vec<String>>);

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

    /// One column per literal kind the backend can express.
    fn create_sql(self, table: &str) -> String {
        if self.is_oracle() {
            format!(
                "CREATE TABLE {table} (\
                 SEQ NUMBER NOT NULL, \
                 NAME VARCHAR2(200), \
                 CODE VARCHAR2(20), \
                 AMT NUMBER(12,2), \
                 HIRED DATE, \
                 TS TIMESTAMP(6), \
                 TSZ TIMESTAMP(6) WITH TIME ZONE, \
                 RAWC RAW(8), \
                 CONSTRAINT {table}_PK PRIMARY KEY (SEQ))"
            )
        } else {
            format!(
                "CREATE TABLE {table} (\
                 SEQ INT NOT NULL, \
                 NAME VARCHAR(200), \
                 CODE VARCHAR(20), \
                 AMT DECIMAL(12,2), \
                 HIRED DATE, \
                 TS DATETIME(6), \
                 FLAG TINYINT, \
                 BINC VARBINARY(16), \
                 PRIMARY KEY (SEQ))"
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

    /// Rows that stress every format at once.
    ///
    /// Deliberately absent: a lone carriage return (Markdown and HTML fold it
    /// into a newline), leading or trailing spaces (a Markdown cell is
    /// trimmed), an empty string (Markdown and HTML spell NULL as an empty
    /// cell, and Oracle stores `''` as NULL anyway), and the literal text
    /// `NULL` (which is exactly what a CSV NULL looks like). Those are the
    /// documented lossy edges, and the unit tests pin each of them.
    fn seed_sql(self, table: &str) -> Vec<String> {
        let hostile =
            "comma, tab\there \"quoted\" it''s pipe|slash\\ <tag> &amp; ]]> 한국어\nsecond line";
        if self.is_oracle() {
            // The seed statement is hand-written SQL, so its own `&` would be
            // read as a substitution variable before it ever reached the
            // server. The production defuser is what puts the character back.
            let hostile = space_query::ui::table_import::defuse_substitution(
                DatabaseType::Oracle,
                &format!("'{hostile}'"),
            );
            vec![
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     1, {hostile}, '00123', 1234.56, \
                     TO_DATE('1980-12-17 09:30:00','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('1980-12-17 09:30:00.123456','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('1980-12-17 09:30:00.123456 +09:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     HEXTORAW('DEADBEEF'))"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     2, NULL, '007', NULL, NULL, NULL, NULL, NULL)"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     3, 'plain', '42', -0.5, \
                     TO_DATE('2024-02-29 23:59:59','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('2024-02-29 23:59:59.000001','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('2024-02-29 23:59:59.000001 -05:30','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     HEXTORAW('00FF'))"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC) VALUES (\
                     4, 'last', '0', 99999999.99, \
                     TO_DATE('1900-01-01 00:00:00','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('1900-01-01 00:00:00.000000','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('1900-01-01 00:00:00.000000 +00:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     NULL)"
                ),
            ]
        } else {
            vec![
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC) VALUES (\
                     1, '{hostile}', '00123', 1234.56, '1980-12-17', \
                     '1980-12-17 09:30:00.123456', 1, 'abc')"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC) VALUES (\
                     2, NULL, '007', NULL, NULL, NULL, NULL, NULL)"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC) VALUES (\
                     3, 'plain', '42', -0.50, '2024-02-29', \
                     '2024-02-29 23:59:59.000001', 0, 'z')"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC) VALUES (\
                     4, 'last', '0', 99999999.99, '1900-01-01', \
                     '1900-01-01 00:00:00.000000', 1, NULL)"
                ),
            ]
        }
    }

    fn select_sql(self, table: &str) -> String {
        format!("SELECT * FROM {table} ORDER BY SEQ")
    }

    fn delete_sql(self, table: &str) -> String {
        format!("DELETE FROM {table}")
    }

    /// Read the declared column types straight out of the catalog, which is
    /// what the import dialog does through `load_table_structure`.
    fn catalog_types_sql(self, table: &str) -> String {
        if self.is_oracle() {
            format!(
                "SELECT column_name, data_type FROM user_tab_columns \
                 WHERE table_name = '{table}' ORDER BY column_id"
            )
        } else {
            // `lower_case_table_names` differs by platform, so match the name
            // case-insensitively rather than guessing how it was stored.
            format!(
                "SELECT COLUMN_NAME, COLUMN_TYPE FROM INFORMATION_SCHEMA.COLUMNS \
                 WHERE TABLE_SCHEMA = DATABASE() AND UPPER(TABLE_NAME) = '{table}' \
                 ORDER BY ORDINAL_POSITION"
            )
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
        let deadline = Instant::now() + Duration::from_secs(120);
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

    fn dispatch(&mut self, sql: &str, script: bool) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        if script {
            self.editor.execute_script_text(sql);
        } else {
            self.editor.execute_sql_text(sql);
        }
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

    fn run(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.dispatch(sql, false)
    }

    /// The path `SqlAction::ExecuteScript` takes, which is how an import runs.
    fn run_script(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.dispatch(sql, true)
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

/// Columns, kinds, and rows exactly as the grid would receive them, with the
/// internal columns the grid never exports dropped.
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

/// Every row of a two-column result, as pairs.
fn pairs(events: &[QueryProgress]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for event in events {
        if let QueryProgress::Rows { rows, .. } = progress_inner(event) {
            for row in rows {
                if row.len() >= 2 {
                    out.push((row[0].clone(), row[1].clone()));
                }
            }
        }
    }
    out
}

/// Whether the grid's text for a cell means SQL NULL.
fn is_null(value: &str) -> bool {
    value.is_empty() || value.eq_ignore_ascii_case(NULL_TEXT)
}

/// What every format must give back for the source rows.
fn expected_cells(rows: &[Vec<String>]) -> Vec<Vec<ImportCell>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|value| normalize_zone_space(&(!is_null(value)).then(|| value.clone())))
                .collect()
        })
        .collect()
}

/// Collapse the space `TO_TIMESTAMP_TZ` puts between a timestamp and its zone
/// offset.
///
/// The SQL Inserts export writes `'… .123456 +09:00'` because the format model
/// `… .FF TZH:TZM` needs the separator; Thin renders the offset with no space
/// and OCI renders it with one. All three spell the same instant and
/// `oracle_temporal_literal` reads all three, so the intermediate text
/// comparison treats them as equal on both sides. The end-to-end comparison
/// against the server is what proves the value.
fn normalize_zone_space(cell: &ImportCell) -> ImportCell {
    let value = cell.as_ref()?;
    let bytes = value.as_bytes();
    if bytes.len() >= 7 {
        let split = bytes.len() - 6;
        let (head, tail) = value.split_at(split);
        let zone_shaped = matches!(tail.as_bytes()[0], b'+' | b'-')
            && tail.as_bytes()[3] == b':'
            && tail[1..3].bytes().all(|b| b.is_ascii_digit())
            && tail[4..6].bytes().all(|b| b.is_ascii_digit());
        if zone_shaped && head.ends_with(' ') {
            return Some(format!("{}{tail}", head.trim_end()));
        }
    }
    Some(value.clone())
}

fn export_grid(columns: &[String], kinds: &[SqlValueKind], rows: &[Vec<String>]) -> ExportGrid {
    ExportGrid {
        columns: columns.to_vec(),
        column_kinds: kinds.to_vec(),
        rows: rows.to_vec(),
        null_text: NULL_TEXT.to_string(),
    }
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

    let _ = h.run(&target.drop_sql(COPY_TABLE));
    let _ = h.run(&target.drop_sql(SOURCE_TABLE));
    h.run(&target.create_sql(SOURCE_TABLE))
        .map_err(|e| format!("create source: {e}"))?;
    h.run(&target.create_sql(COPY_TABLE))
        .map_err(|e| format!("create copy: {e}"))?;
    for sql in target.seed_sql(SOURCE_TABLE) {
        h.run(&sql).map_err(|e| format!("seed: {e}"))?;
    }
    let _ = h.run("COMMIT");

    let select_events = h
        .run(&target.select_sql(SOURCE_TABLE))
        .map_err(|e| format!("select source: {e}"))?;
    let (columns, kinds, rows) =
        grid_view(&select_events).ok_or_else(|| "SELECT produced no grid columns".to_string())?;
    if rows.len() != 4 {
        return Err(format!("expected 4 source rows, got {}", rows.len()));
    }
    println!("source columns and driver-classified kinds:");
    for (column, kind) in columns.iter().zip(kinds.iter()) {
        println!("  {column:<6} {kind:?}");
    }

    // (1) The declared type in the catalog has to produce the same literal as
    //     the type the driver reported. The import reads the catalog; the
    //     export reads the driver; a disagreement that changes a literal would
    //     make the round trip lie.
    let catalog_events = h
        .run(&target.catalog_types_sql(COPY_TABLE))
        .map_err(|e| format!("catalog types: {e}"))?;
    let catalog = pairs(&catalog_events);
    if catalog.len() != columns.len() {
        return Err(format!(
            "catalog reported {} columns, the driver reported {}",
            catalog.len(),
            columns.len()
        ));
    }
    let mut targets: Vec<TargetColumn> = Vec::new();
    for (index, (name, data_type)) in catalog.iter().enumerate() {
        let catalog_kind = column_kind_for_data_type(db_type, data_type);
        let driver_kind = kinds.get(index).copied().unwrap_or(SqlValueKind::Unknown);
        println!(
            "  {name:<6} declared {data_type:<32} -> {catalog_kind:?} (driver {driver_kind:?})"
        );
        for row in &rows {
            let Some(value) = row.get(index) else {
                continue;
            };
            if is_null(value) {
                continue;
            }
            let from_catalog = sql_literal_for_value(db_type, catalog_kind, value);
            let from_driver = sql_literal_for_value(db_type, driver_kind, value);
            if from_catalog != from_driver {
                return Err(format!(
                    "column {name}: the declared type gives {from_catalog} but the driver type \
                     gives {from_driver}"
                ));
            }
        }
        targets.push(TargetColumn {
            name: name.clone(),
            kind: catalog_kind,
            nullable: true,
        });
    }
    println!("PASS: every declared column type renders the same literal as the driver's");

    let grid = export_grid(&columns, &kinds, &rows);
    let expected = expected_cells(&rows);
    let directory = std::env::temp_dir().join("verify_import_live");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;

    for format in ExportFormat::ALL {
        println!("\n----- {} -----", format.label());

        // Write the file exactly the way the app writes it.
        let body = if format == ExportFormat::SqlInserts {
            build_sql_inserts(&GridSqlSelection {
                db_type,
                table: Some(SOURCE_TABLE.to_string()),
                all_columns: columns.clone(),
                column_kinds: kinds.clone(),
                selected_columns: (0..columns.len()).collect(),
                rows: rows.clone(),
                null_text: NULL_TEXT.to_string(),
            })
        } else {
            render(format, &grid)
        };
        let path = directory.join(format!("rows.{}", format.extension()));
        std::fs::write(&path, format!("{}{body}", format.file_byte_order_mark()))
            .map_err(|e| format!("{}: {e}", path.display()))?;

        // The extension has to name the format, because that is what preselects
        // it in the dialog.
        if detect_format(&path) != Some(format) {
            return Err(format!(
                "{} wrote {} but the extension detects {:?}",
                format.label(),
                path.display(),
                detect_format(&path)
            ));
        }

        // Read it back off disk, the way the import does.
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let options = ImportOptions {
            format,
            has_header: true,
            null_text: NULL_TEXT.to_string(),
        };
        let parsed =
            parse(&text, &options).map_err(|e| format!("{} parse: {e}", format.label()))?;
        let parsed_cells: Vec<Vec<ImportCell>> = parsed
            .rows
            .iter()
            .map(|row| row.iter().map(normalize_zone_space).collect())
            .collect();
        if parsed_cells != expected {
            return Err(format!(
                "{} parsed differently from the source\n  expected: {expected:?}\n  parsed:   {parsed_cells:?}"
                ,
                format.label()
            ));
        }

        // Every source column has to find its target by name.
        let mapping = default_mapping(&parsed.columns, &targets);
        if mapping.iter().any(Option::is_none) {
            return Err(format!(
                "{}: {:?} did not all map onto {:?}",
                format.label(),
                parsed.columns,
                targets.iter().map(|t| &t.name).collect::<Vec<_>>()
            ));
        }

        let request = ImportRequest {
            db_type,
            table: COPY_TABLE,
            targets: &targets,
            mapping: &mapping,
            data: &parsed,
            batch_rows: BATCH_ROWS,
        };
        let script =
            build_insert_script(&request).map_err(|e| format!("{} script: {e}", format.label()))?;
        println!("{}", describe(&request).unwrap_or_default());
        let statements = script.matches(';').count();
        if statements < 2 {
            return Err(format!(
                "{}: expected a multi-statement script at batch size {BATCH_ROWS}, got {statements}",
                format.label()
            ));
        }

        h.run(&target.delete_sql(COPY_TABLE))
            .map_err(|e| format!("{} clear copy: {e}", format.label()))?;
        h.run_script(&script).map_err(|e| {
            format!(
                "{} import failed: {e}\n--- script ---\n{script}",
                format.label()
            )
        })?;
        let _ = h.run("COMMIT");

        let copy_events = h
            .run(&target.select_sql(COPY_TABLE))
            .map_err(|e| format!("{} select copy: {e}", format.label()))?;
        let (_, _, copy_rows) = grid_view(&copy_events)
            .ok_or_else(|| format!("{}: copy SELECT produced no columns", format.label()))?;
        if copy_rows != rows {
            return Err(format!(
                "{} did not round-trip\n  source: {rows:?}\n  copy:   {copy_rows:?}",
                format.label()
            ));
        }
        println!(
            "PASS: {} round-tripped {} rows through {} statements",
            format.label(),
            rows.len(),
            statements
        );
    }

    let _ = h.run(&target.drop_sql(COPY_TABLE));
    let _ = h.run(&target.drop_sql(SOURCE_TABLE));
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
