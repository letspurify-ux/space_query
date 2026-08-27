#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for file → table import, across every supported backend:
// Oracle Thin, Oracle OCI, MySQL, and MariaDB.
//
// The unit tests in `src/ui/result_import.rs` and `src/ui/table_import.rs`
// prove the parsers and the SQL builder against text the same process wrote.
// They cannot prove the two things that only a server can settle:
//
//   (1) that a column's *declared* type — what
//       `SqlValueKind::for_declared_type` reads out of the catalog — produces the
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
// After the round-trip loop it also checks the one thing about an import that
// belongs to the transaction model rather than to the parsers: an import runs
// through `SqlAction::ExecuteScript` on the active tab, so a tab pinned READ
// ONLY must refuse it and leave the target table empty, and unpinning must let
// the very same script through.
//
// Usage: verify_import_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time.

use fltk::{app, input::IntInput};
use mysql::{Conn as MysqlConn, Opts as MysqlOpts};
use oracle::Connection as OracleConnection;
use space_query::db::query::mysql_executor::MysqlObjectBrowser;
use space_query::db::query::ObjectBrowser;
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode, QueryCell, QueryResult,
    SqlValueKind, TableColumnDetail, TransactionAccessMode, TransactionIsolation, TransactionMode,
};
use space_query::ui::grid_sql_export::{
    build_sql_inserts, sql_literal_for_value, GridSqlSelection, SqlWriteDialect,
};
use space_query::ui::object_browser::ObjectBrowserWidget;
use space_query::ui::result_export::{
    render, ExportDestination, ExportFormat, ExportGrid, ExportScope,
};
use space_query::ui::result_export_dialog::ExportChoice;
use space_query::ui::result_import::ImportedTable;
use space_query::ui::result_import::{detect_format, parse, ImportCell, ImportOptions};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use space_query::ui::table_import::{
    build_insert_script, default_mapping, describe, ImportRequest, TargetColumn, DEFAULT_BATCH_ROWS,
};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

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
    ///
    /// `BLOBC` and `BITC` are here because they are the two declared types
    /// whose catalog classification used to disagree with the driver's: the
    /// catalog called an Oracle `BLOB` binary (so an import wrote
    /// `HEXTORAW(<file text>)`) and a MySQL `BIT` a number (so an import wrote
    /// `1` where the grid's own text means 49). Both are `Unknown` now, on both
    /// roads, and check (1) below is what compares them for real.
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
                 BLOBC BLOB, \
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
                 BITC BIT(8), \
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
    /// The `BLOB` and `BIT` values are deliberately printable ASCII: both reach
    /// the grid as a lossy rendering of their BYTES, so only a byte that
    /// survives UTF-8 and the markup grammars can round-trip through every file
    /// format. That is the honest limit of those two types here, not a property
    /// of the import.
    ///
    /// `BITC` is `b'00110001'` on purpose — byte 0x31, which the grid shows as
    /// the DIGIT `1`. That is the one shape that tells the two readings of a
    /// `BIT` column apart: read as a number, `1` goes back as the bit value 1
    /// and the row is silently rewritten from 49; read as what it is — a lossy
    /// byte rendering — `'1'` goes back as 49, which is what MySQL stored.
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
                SqlWriteDialect::family_default(DatabaseType::Oracle),
                &format!("'{hostile}'"),
            );
            vec![
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC, BLOBC) VALUES (\
                     1, {hostile}, '00123', 1234.56, \
                     TO_DATE('1980-12-17 09:30:00','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('1980-12-17 09:30:00.123456','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('1980-12-17 09:30:00.123456 +09:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     HEXTORAW('DEADBEEF'), HEXTORAW('0102FF'))"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC, BLOBC) VALUES (\
                     2, NULL, '007', NULL, NULL, NULL, NULL, NULL, NULL)"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC, BLOBC) VALUES (\
                     3, 'plain', '42', -0.5, \
                     TO_DATE('2024-02-29 23:59:59','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('2024-02-29 23:59:59.000001','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('2024-02-29 23:59:59.000001 -05:30','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     HEXTORAW('00FF'), HEXTORAW('41'))"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, TSZ, RAWC, BLOBC) VALUES (\
                     4, 'last', '0', 99999999.99, \
                     TO_DATE('1900-01-01 00:00:00','YYYY-MM-DD HH24:MI:SS'), \
                     TO_TIMESTAMP('1900-01-01 00:00:00.000000','YYYY-MM-DD HH24:MI:SS.FF'), \
                     TO_TIMESTAMP_TZ('1900-01-01 00:00:00.000000 +00:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'), \
                     NULL, NULL)"
                ),
            ]
        } else {
            vec![
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC, BITC) VALUES (\
                     1, '{hostile}', '00123', 1234.56, '1980-12-17', \
                     '1980-12-17 09:30:00.123456', 1, 'abc', b'00110001')"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC, BITC) VALUES (\
                     2, NULL, '007', NULL, NULL, NULL, NULL, NULL, NULL)"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC, BITC) VALUES (\
                     3, 'plain', '42', -0.50, '2024-02-29', \
                     '2024-02-29 23:59:59.000001', 0, 'z', b'01011010')"
                ),
                format!(
                    "INSERT INTO {table} (SEQ, NAME, CODE, AMT, HIRED, TS, FLAG, BINC, BITC) VALUES (\
                     4, 'last', '0', 99999999.99, '1900-01-01', \
                     '1900-01-01 00:00:00.000000', 1, NULL, NULL)"
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
    /// `1` when `table` is still here — the question an injection check asks.
    fn table_exists_sql(self, table: &str) -> String {
        if self.is_oracle() {
            format!("SELECT COUNT(*) AS N FROM USER_TABLES WHERE TABLE_NAME = '{table}'")
        } else {
            format!(
                "SELECT COUNT(*) AS N FROM INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{table}'"
            )
        }
    }

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
    shared: space_query::db::SharedConnection,
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

    /// The toolbar write path in full: `update_transaction_mode_from_controls`
    /// pins the tab AND pushes the change onto the tab's retained session.
    /// Pinning alone never reaches a session the tab is already holding.
    fn set_transaction_mode_like_the_toolbar(
        &mut self,
        mode: TransactionMode,
    ) -> Result<(), String> {
        self.editor.set_tab_transaction_mode(mode);
        let (target, default_transaction_isolation) = {
            let guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.retained_session_target(),
                guard.default_transaction_isolation(),
            )
        };
        let outcome = self.editor.apply_transaction_mode_to_retained_session(
            target,
            mode,
            default_transaction_isolation,
            "verify import",
        );
        match outcome {
            space_query::db::RetainedSessionMutationOutcome::Applied
            | space_query::db::RetainedSessionMutationOutcome::AppliedWithWarning(_)
            | space_query::db::RetainedSessionMutationOutcome::NoSession => Ok(()),
            other => Err(format!(
                "the tab was pinned but its retained session refused the mode: {other:?}"
            )),
        }
    }

    /// Same path, but a failing statement is the expected outcome rather than
    /// an error to bail out on.
    fn run_script_expecting_failure(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_script_text(sql);
        let done = Arc::clone(&self.done);
        self.pump_until("script to finish", || done.load(Ordering::SeqCst))?;
        Ok(self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone())
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

/// The single number a `SELECT COUNT(*) AS N` came back with.
fn single_count(events: &[QueryProgress]) -> Option<String> {
    grid_view(events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .map(|value| value.trim().to_string())
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
        // The rows come off a real grid, where a NULL is already the NULL
        // display text; the export snapshot makes that an absent cell.
        rows: rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| (value != NULL_TEXT).then(|| value.clone()))
                    .collect()
            })
            .collect(),
        null_text: NULL_TEXT.to_string(),
    }
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());
    let info = target.connection_info();
    let db_type = info.db_type;
    // The rules the SESSION will run under, taken before the info is consumed.
    let dialect = SqlWriteDialect::for_connection(&info);

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
        shared: Arc::clone(&shared),
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
        let catalog_kind = SqlValueKind::for_declared_type(db_type, data_type);
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
            // Compared as the writer ANSWERS, refusal included: two kinds
            // that refuse the same value agree, and one that refuses where the
            // other writes is exactly the disagreement this checks for.
            let from_catalog = sql_literal_for_value(dialect, catalog_kind, value);
            let from_driver = sql_literal_for_value(dialect, driver_kind, value);
            if from_catalog != from_driver {
                return Err(format!(
                    "column {name}: the declared type gives {from_catalog:?} but the driver type \
                     gives {from_driver:?}"
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

    // Kept so the read-only scenario below can re-run a real import script.
    let mut last_script: Option<String> = None;
    for format in ExportFormat::ALL {
        println!("\n----- {} -----", format.label());

        // Write the file exactly the way the app writes it.
        let body = if format == ExportFormat::SqlInserts {
            build_sql_inserts(&GridSqlSelection {
                dialect,
                table: Some(SOURCE_TABLE.to_string()),
                all_columns: columns.clone(),
                column_kinds: kinds.clone(),
                selected_columns: (0..columns.len()).collect(),
                rows: grid.rows.clone(),
            })
            .into_parts()
            .map_err(|reason| format!("SQL Inserts was refused: {reason}"))?
            .0
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
            dialect,
            table: COPY_TABLE,
            targets: &targets,
            mapping: &mapping,
            data: &parsed,
            batch_rows: BATCH_ROWS,
        };
        let script =
            build_insert_script(&request).map_err(|e| format!("{} script: {e}", format.label()))?;
        last_script = Some(script.clone());
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

    // An import is a script on the ACTIVE tab, so the tab's transaction mode
    // governs it. A tab pinned READ ONLY must refuse the whole thing and leave
    // the target empty; unpinning must let the identical script through.
    let script = last_script.ok_or("no import script was built")?;
    h.run(&target.delete_sql(COPY_TABLE))
        .map_err(|e| format!("clear copy before the read-only import: {e}"))?;
    let _ = h.run("COMMIT");
    // The toolbar refuses a transaction-mode change on a session that still
    // needs a decision; clear it the way the app tells the user to, so this
    // scenario tests the read-only promise rather than that gate.
    let _ = h.editor.discard_pooled_session_for_close();
    h.set_transaction_mode_like_the_toolbar(TransactionMode::new(
        TransactionIsolation::Default,
        TransactionAccessMode::ReadOnly,
    ))?;
    let refused_events = h.run_script_expecting_failure(&script)?;
    let refusal = first_error(&refused_events).ok_or(
        "BUG: an import script ran to completion on a tab pinned READ ONLY (no statement failed)",
    )?;
    let needles: &[&str] = if target.is_oracle() {
        &["read-only mode blocks", "ora-01456"]
    } else {
        &["read only"]
    };
    if !needles.iter().any(|needle| {
        refusal
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }) {
        return Err(format!(
            "the read-only import failed for the wrong reason: {refusal}"
        ));
    }
    let _ = h.run("ROLLBACK");
    // Read back on the same session, so an uncommitted leak is visible too.
    let leak_events = h
        .run(&format!("SELECT COUNT(*) AS N FROM {COPY_TABLE}"))
        .map_err(|e| format!("count after the read-only import: {e}"))?;
    let leaked = grid_view(&leak_events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    if leaked.trim() != "0" {
        return Err(format!(
            "BUG: the refused import still landed rows (COUNT(*) = {leaked:?})"
        ));
    }
    let _ = h.run("ROLLBACK");
    println!("PASS: a READ ONLY tab refused the import and left the target empty");

    // Back on Read write through the same toolbar path.
    let _ = h.editor.discard_pooled_session_for_close();
    h.set_transaction_mode_like_the_toolbar(TransactionMode::default())?;
    h.editor.clear_tab_transaction_mode_override();
    h.run_script(&script)
        .map_err(|e| format!("unpinned import failed: {e}"))?;
    let _ = h.run("COMMIT");
    let restored_events = h
        .run(&format!("SELECT COUNT(*) AS N FROM {COPY_TABLE}"))
        .map_err(|e| format!("count after the unpinned import: {e}"))?;
    let restored = grid_view(&restored_events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    if restored.trim() == "0" {
        return Err("unpinning did not let the same import script through".into());
    }
    println!("PASS: the same import script succeeds once the pin is removed ({restored} rows)");

    // An import must obey the tab's auto-commit too: pinned ON, the imported
    // rows have to survive a later ROLLBACK. (A grid save is covered by
    // verify_grid_save_live; the import runs as its own script instead.)
    h.run(&target.delete_sql(COPY_TABLE))
        .map_err(|e| format!("clear copy before the auto-commit import: {e}"))?;
    let _ = h.run("COMMIT");
    h.editor.set_tab_auto_commit(true);
    h.run_script(&script)
        .map_err(|e| format!("import on an auto-commit tab failed: {e}"))?;
    h.editor.set_tab_auto_commit(false);
    let _ = h.run("ROLLBACK");
    let durable_events = h
        .run(&format!("SELECT COUNT(*) AS N FROM {COPY_TABLE}"))
        .map_err(|e| format!("count after the auto-commit import: {e}"))?;
    let durable = grid_view(&durable_events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    if durable.trim() == "0" {
        return Err(
            "BUG: an import on an auto-commit tab did not commit (COUNT(*) = 0 after ROLLBACK)"
                .into(),
        );
    }
    println!("PASS: an import on an auto-commit tab survived a later ROLLBACK ({durable} rows)");

    // Asked while the source table is still here: this read is the object
    // browser's own, on a connection of its own, and it has to bring back the
    // same rows the grid does.
    println!("\n----- the rows `Export Data...` reads -----");
    let export_read = verify_table_export_read(target);

    let _ = h.run(&target.drop_sql(COPY_TABLE));
    let _ = h.run(&target.drop_sql(SOURCE_TABLE));
    let _ = h.run("COMMIT");
    export_read?;

    println!("\n----- computed columns, hostile literals, awkward names -----");
    verify_a_production_sized_batch(target, &mut h)?;
    verify_a_file_cell_cannot_inject_a_statement(target, &mut h)?;
    verify_generated_columns(target, &mut h)?;
    verify_hostile_sql_round_trip(target, &mut h)?;
    verify_awkward_table_name(target, &mut h)?;
    verify_no_backslash_escapes(target, &mut h)?;
    verify_batched_script_reimport(target, &mut h)?;

    println!("\n----- round 8: the NULL text, blank lines, and a value no literal holds -----");
    verify_null_text_and_blank_lines(target, &mut h)?;
    verify_a_value_no_literal_can_hold(target, &mut h)?;

    println!("\n----- round 9: a column `SELECT *` leaves out -----");
    verify_invisible_columns(target, &mut h)?;
    Ok(())
}

/// A table with an INVISIBLE column in the middle.
const INVISIBLE_TABLE: &str = "OQT_IMPORT_INVIS";

/// A column no `SELECT *` returns claims no position in a file.
///
/// The other half of the root round 8 fixed. A headerless file means "the
/// table's columns, in order" — but the file is a `SELECT *`, and an INVISIBLE
/// column (Oracle 12c+, MySQL 8.0.23+, MariaDB 10.3+) is a column a statement
/// may name and no `SELECT *` returns.
///
/// Measured before the fix: MySQL 8.0 and MariaDB 12.2 keep the column's
/// `ORDINAL_POSITION`, so the catalog listed `A, B, C` where the file held
/// `A, C` — and the file's second value went into B. Oracle sorts it last
/// (`COLUMN_ID` is NULL) and happened to line up; this checks that it still
/// does, for the same reason the other two now do.
fn verify_invisible_columns(target: Target, h: &mut Harness) -> Result<(), String> {
    let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {INVISIBLE_TABLE} (A NUMBER, B NUMBER INVISIBLE, C NUMBER)")
    } else {
        format!("CREATE TABLE {INVISIBLE_TABLE} (A INT, B INT INVISIBLE, C INT)")
    };
    h.run(&create)
        .map_err(|e| format!("create a table with an invisible column: {e}"))?;
    let _ = h.run("COMMIT");

    // What the file will hold: whatever `SELECT *` returns.
    let star = read_table_rows(target, &format!("SELECT * FROM {INVISIBLE_TABLE}"))?;
    let star_columns: Vec<String> = star
        .columns
        .iter()
        .map(|column| column.name.trim().to_ascii_uppercase())
        .collect();
    println!("  SELECT * returns {star_columns:?}");
    if star_columns != ["A", "C"] {
        let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
        return Err(format!(
            "this server does not hide an INVISIBLE column from SELECT *: {star_columns:?}"
        ));
    }

    let columns = read_table_structure(target, INVISIBLE_TABLE)?;
    let db_type = target.connection_info().db_type;
    let table_columns = ObjectBrowserWidget::import_targets(db_type, &columns);
    let targets = table_columns.writable();
    println!(
        "  catalog reports {:?}, targets {:?}",
        columns
            .iter()
            .map(|column| (column.name.clone(), column.is_invisible))
            .collect::<Vec<_>>(),
        targets
            .iter()
            .map(|target| target.name.clone())
            .collect::<Vec<_>>()
    );
    // An invisible column is still a target: a statement may name it.
    if !targets
        .iter()
        .any(|target| target.name.eq_ignore_ascii_case("B"))
    {
        let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
        return Err("an invisible column must stay an import target".to_string());
    }

    // A headerless file of the SELECT * columns: two values, and the second
    // one is C's.
    let data = ImportedTable {
        columns: vec!["COLUMN_1".to_string(), "COLUMN_2".to_string()],
        rows: vec![vec![Some("100".to_string()), Some("300".to_string())]],
    };
    let mapping = table_columns.positional_mapping(data.columns.len());
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: INVISIBLE_TABLE,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the headerless import script: {e}"))?;
    println!("  {}", script.trim());
    h.run_script(&script)
        .map_err(|e| format!("the headerless import failed: {e}"))?;
    let _ = h.run("COMMIT");

    let counted = h.run(&format!(
        "SELECT COUNT(*) AS N FROM {INVISIBLE_TABLE} WHERE A = 100 AND C = 300 AND B IS NULL"
    ))?;
    if single_count(&counted).as_deref() != Some("1") {
        let rows = h.run(&format!("SELECT A, B, C FROM {INVISIBLE_TABLE}"))?;
        let landed = grid_view(&rows).map(|(_, _, rows)| rows);
        let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
        return Err(format!(
            "the file's second value did not reach C — the row landed as {landed:?}"
        ));
    }
    println!("PASS: a column SELECT * omits takes no file position, and stays a target");

    // The premise behind keeping it on offer: naming it explicitly works.
    h.run(&format!(
        "INSERT INTO {INVISIBLE_TABLE} (A, B, C) VALUES (1, 2, 3)"
    ))
    .map_err(|e| format!("an explicit INSERT into an invisible column failed: {e}"))?;
    let _ = h.run("COMMIT");
    let counted = h.run(&format!(
        "SELECT COUNT(*) AS N FROM {INVISIBLE_TABLE} WHERE B = 2"
    ))?;
    if single_count(&counted).as_deref() != Some("1") {
        let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
        return Err("an explicit value did not reach the invisible column".to_string());
    }
    println!("PASS: an invisible column really is writable by name");

    // The catalog fact is read by THREE Oracle statements — OCI's structure
    // read, thin's, and `DESCRIBE` — and only the first two are on the path
    // above. `DESC` is how the third one runs.
    if target.is_oracle() {
        let described = h
            .run(&format!("DESC {INVISIBLE_TABLE}"))
            .map_err(|e| format!("DESCRIBE reads the same catalog columns: {e}"))?;
        let names: Vec<String> = grid_view(&described)
            .map(|(_, _, rows)| {
                rows.iter()
                    .filter_map(|row| row.first().cloned())
                    .map(|name| name.trim().to_ascii_uppercase())
                    .collect()
            })
            .unwrap_or_default();
        if !names.iter().any(|name| name == "A") {
            let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
            return Err(format!("DESCRIBE returned {names:?}"));
        }
        println!("PASS: DESCRIBE reads the same catalog columns ({names:?})");
    }

    let _ = h.run(&target.drop_sql(INVISIBLE_TABLE));
    let _ = h.run("COMMIT");
    Ok(())
}

/// A table of one text column, for the NULL-text round trip.
const NULL_TEXT_TABLE: &str = "OQT_IMPORT_NULLTEXT";
/// A table with a wide binary column, for the literal-limit refusal.
const LONG_VALUE_TABLE: &str = "OQT_IMPORT_LONGVAL";

/// SQL NULL and the four letters `NULL` are different values, and a BLANK LINE
/// is not a row.
///
/// Both ends of a delimited file, through the production writer and reader and
/// a real server. Before this round:
///
/// * the writer wrote the same bytes for a NULL and for a value spelling the
///   NULL text, so the reader — which can only see the bytes — turned the
///   string into SQL NULL, on every backend;
/// * a trailing blank line was read as a record of one empty field, padded out
///   to the file's width, and imported as an extra row.
fn verify_null_text_and_blank_lines(target: Target, h: &mut Harness) -> Result<(), String> {
    let _ = h.run(&target.drop_sql(NULL_TEXT_TABLE));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {NULL_TEXT_TABLE} (SEQ NUMBER, V VARCHAR2(20))")
    } else {
        format!("CREATE TABLE {NULL_TEXT_TABLE} (SEQ INT, V VARCHAR(20))")
    };
    h.run(&create)
        .map_err(|e| format!("create the NULL-text table: {e}"))?;
    for (seq, value) in [(1, "NULL"), (2, "plain")] {
        h.run(&format!(
            "INSERT INTO {NULL_TEXT_TABLE} (SEQ, V) VALUES ({seq}, '{value}')"
        ))
        .map_err(|e| format!("seed the NULL-text table: {e}"))?;
    }
    h.run(&format!(
        "INSERT INTO {NULL_TEXT_TABLE} (SEQ, V) VALUES (3, NULL)"
    ))
    .map_err(|e| format!("seed a real NULL: {e}"))?;
    let _ = h.run("COMMIT");

    // Read the way `Export Data...` reads: through the object-browser path,
    // where a NULL is still the driver's own marker. The editor's grid holds
    // the session's NULL display text for it, and a value spelling those same
    // letters is that text too — which is the documented limit of a GRID
    // export, and not what this check is about.
    let result = read_table_rows(
        target,
        &format!("SELECT SEQ, V FROM {NULL_TEXT_TABLE} ORDER BY SEQ"),
    )?;
    if result.rows.len() != 3 {
        return Err(format!("expected 3 rows, got {}", result.rows.len()));
    }
    let value_of = |row: usize| -> Option<String> {
        let cell = result.rows.get(row)?.get(1)?;
        (!QueryCell::is_null_result_text(cell)).then(|| cell.clone())
    };
    if value_of(0).as_deref() != Some("NULL") || value_of(2).is_some() {
        return Err(format!(
            "the export read did not tell the string from the NULL: {:?}",
            result.rows
        ));
    }

    for format in [ExportFormat::Csv, ExportFormat::Tsv] {
        // The production renderer for a tree export, byte for byte.
        let delivery = ObjectBrowserWidget::render_table_export(
            NULL_TEXT_TABLE,
            SqlWriteDialect::for_connection(&target.connection_info()),
            ExportChoice {
                format,
                scope: ExportScope::All,
                destination: ExportDestination::File,
            },
            &result,
            &[],
        );
        let body = delivery
            .content
            .into_parts()
            .map_err(|reason| format!("{}: {reason}", format.label()))?
            .0;
        // A file that ends with a blank line — what an editor leaves behind.
        // The byte-order mark goes the way `parse` strips it.
        let text = format!("{body}\n");
        let parsed = parse(
            &text,
            &ImportOptions {
                format,
                has_header: true,
                null_text: NULL_TEXT.to_string(),
            },
        )
        .map_err(|e| format!("{}: {e}", format.label()))?;
        if parsed.rows.len() != 3 {
            return Err(format!(
                "{}: a blank line became a row — {} rows parsed from 3",
                format.label(),
                parsed.rows.len()
            ));
        }
        if parsed.rows[0][1] != Some("NULL".to_string()) {
            return Err(format!(
                "{}: the string NULL came back as {:?}",
                format.label(),
                parsed.rows[0][1]
            ));
        }
        if parsed.rows[2][1].is_some() {
            return Err(format!(
                "{}: a real NULL came back as {:?}",
                format.label(),
                parsed.rows[2][1]
            ));
        }

        // And the whole trip through a server: import into a cleared table and
        // ask the SERVER which rows are NULL.
        let targets = vec![
            TargetColumn {
                name: "SEQ".to_string(),
                kind: SqlValueKind::Number,
                nullable: true,
            },
            TargetColumn {
                name: "V".to_string(),
                kind: SqlValueKind::String,
                nullable: true,
            },
        ];
        let mapping = default_mapping(&parsed.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::for_connection(&target.connection_info()),
            table: NULL_TEXT_TABLE,
            targets: &targets,
            mapping: &mapping,
            data: &parsed,
            batch_rows: BATCH_ROWS,
        })
        .map_err(|e| format!("{}: build the script: {e}", format.label()))?;
        h.run(&target.delete_sql(NULL_TEXT_TABLE))
            .map_err(|e| format!("{}: clear: {e}", format.label()))?;
        h.run_script(&script)
            .map_err(|e| format!("{}: import: {e}", format.label()))?;
        let _ = h.run("COMMIT");

        let counted = h
            .run(&format!(
                "SELECT COUNT(*) AS N FROM {NULL_TEXT_TABLE} WHERE V IS NULL"
            ))
            .map_err(|e| format!("{}: count NULLs: {e}", format.label()))?;
        if single_count(&counted).as_deref() != Some("1") {
            return Err(format!(
                "{}: the server sees {:?} NULL rows, expected exactly 1",
                format.label(),
                single_count(&counted)
            ));
        }
        let counted = h
            .run(&format!(
                "SELECT COUNT(*) AS N FROM {NULL_TEXT_TABLE} WHERE V = 'NULL'"
            ))
            .map_err(|e| format!("{}: count the string: {e}", format.label()))?;
        if single_count(&counted).as_deref() != Some("1") {
            return Err(format!(
                "{}: the server sees {:?} rows holding the text NULL, expected exactly 1",
                format.label(),
                single_count(&counted)
            ));
        }
        let counted = h
            .run(&format!("SELECT COUNT(*) AS N FROM {NULL_TEXT_TABLE}"))
            .map_err(|e| format!("{}: count rows: {e}", format.label()))?;
        if single_count(&counted).as_deref() != Some("3") {
            return Err(format!(
                "{}: the table holds {:?} rows, expected 3 — a blank line was imported",
                format.label(),
                single_count(&counted)
            ));
        }
        println!(
            "PASS: {} keeps a real NULL, the text NULL, and no blank-line row",
            format.label()
        );
    }

    let _ = h.run(&target.drop_sql(NULL_TEXT_TABLE));
    let _ = h.run("COMMIT");
    Ok(())
}

/// A value no single literal can carry is refused by name, before anything runs.
///
/// Oracle only, because Oracle is the only backend with a per-literal limit:
/// 4000 bytes of content, `ORA-01704` past it, whatever the target column is.
/// A 3000-byte `BLOB` reaches the grid as 6000 hex characters, and this app's
/// own `SQL Inserts` export used to write a file that answered `ORA-01704` when
/// re-imported. The full `RAW(2000)` beside it — exactly 4000 hex characters —
/// still has to be written, because the server takes it.
fn verify_a_value_no_literal_can_hold(target: Target, h: &mut Harness) -> Result<(), String> {
    let dialect = SqlWriteDialect::for_connection(&target.connection_info());
    let at_limit = "AB".repeat(2000);
    if at_limit.len() != 4000 {
        return Err("the fixture is not 4000 hex characters".to_string());
    }

    if !target.is_oracle() {
        // The MySQL family has no per-literal limit, so the same value is
        // written — what bites there is the packet, which the batch bounds.
        let long = "z".repeat(6000);
        for kind in [SqlValueKind::Unknown, SqlValueKind::Binary] {
            sql_literal_for_value(dialect, kind, &long).map_err(|reason| {
                format!("the MySQL family refused a {kind:?} value: {reason:?}")
            })?;
        }
        println!("PASS: this family has no per-literal limit and writes the value");
        return Ok(());
    }

    // What the writer says, before a server is asked.
    sql_literal_for_value(dialect, SqlValueKind::Binary, &at_limit)
        .map_err(|reason| format!("a full RAW(2000) was refused: {reason:?}"))?;
    let over = format!("{at_limit}CD");
    if sql_literal_for_value(dialect, SqlValueKind::Binary, &over).is_ok() {
        return Err("a value past the literal limit was written anyway".to_string());
    }

    // And what the server says about the one that IS written.
    let _ = h.run(&target.drop_sql(LONG_VALUE_TABLE));
    h.run(&format!(
        "CREATE TABLE {LONG_VALUE_TABLE} (ID NUMBER, R RAW(2000), B BLOB)"
    ))
    .map_err(|e| format!("create the long-value table: {e}"))?;
    let literal = sql_literal_for_value(dialect, SqlValueKind::Binary, &at_limit)
        .map_err(|reason| format!("{reason:?}"))?;
    h.run(&format!(
        "INSERT INTO {LONG_VALUE_TABLE} (ID, R) VALUES (1, {literal})"
    ))
    .map_err(|e| format!("the server refused a full RAW(2000) literal: {e}"))?;
    let _ = h.run("COMMIT");
    println!("PASS: a full RAW(2000) is still written and the server takes it");

    // A BLOB past 2000 bytes: the grid holds its hex, and the import refuses
    // the whole script by name rather than sending one the server will break.
    // Built in PL/SQL because no literal can carry it — which is the point.
    h.run(&format!(
        "DECLARE b BLOB; BEGIN DBMS_LOB.CREATETEMPORARY(b, TRUE); \
         FOR i IN 1..3 LOOP DBMS_LOB.APPEND(b, TO_BLOB(UTL_RAW.CAST_TO_RAW(RPAD('A', 1000, 'A')))); \
         END LOOP; INSERT INTO {LONG_VALUE_TABLE} (ID, B) VALUES (2, b); END;"
    ))
    .map_err(|e| format!("seed a 3000-byte BLOB: {e}"))?;
    let _ = h.run("COMMIT");

    let events = h
        .run(&format!(
            "SELECT ID, B FROM {LONG_VALUE_TABLE} WHERE ID = 2"
        ))
        .map_err(|e| format!("select the BLOB: {e}"))?;
    let (columns, kinds, rows) =
        grid_view(&events).ok_or_else(|| "the BLOB SELECT produced no columns".to_string())?;
    let blob_text = rows
        .first()
        .and_then(|row| row.get(1))
        .cloned()
        .unwrap_or_default();
    if blob_text.len() != 6000 {
        return Err(format!(
            "the grid holds {} characters ({blob_text:.40}) for a 3000-byte BLOB, expected 6000",
            blob_text.len()
        ));
    }

    // The export half: the file is refused, not written.
    let grid = export_grid(&columns, &kinds, &rows);
    let refusal = build_sql_inserts(&GridSqlSelection {
        dialect,
        table: Some(LONG_VALUE_TABLE.to_string()),
        all_columns: columns.clone(),
        column_kinds: kinds.clone(),
        selected_columns: (0..columns.len()).collect(),
        rows: grid.rows.clone(),
    });
    let Some(reason) = refusal.refusal() else {
        return Err("the SQL Inserts export wrote a file no server can run".to_string());
    };
    if !reason.contains("Row 1") || !reason.contains('B') {
        return Err(format!("the refusal does not name the cell: {reason}"));
    }
    println!("PASS: the export refuses the BLOB by name — {reason}");

    // The import half: the same value, from a file, refused before it runs.
    let targets = vec![TargetColumn {
        name: "B".to_string(),
        kind: SqlValueKind::Unknown,
        nullable: true,
    }];
    let data = ImportedTable {
        columns: vec!["B".to_string()],
        rows: vec![vec![Some(blob_text)]],
    };
    let mapping = default_mapping(&data.columns, &targets);
    match build_insert_script(&ImportRequest {
        dialect,
        table: LONG_VALUE_TABLE,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    }) {
        Ok(script) => {
            let outcome = h.run_script_expecting_failure(&script)?;
            return Err(format!(
                "the import built a script the server answered with {:?}",
                first_error(&outcome)
            ));
        }
        Err(reason) => println!("PASS: the import refuses it before anything runs — {reason}"),
    }

    let _ = h.run(&target.drop_sql(LONG_VALUE_TABLE));
    let _ = h.run("COMMIT");
    Ok(())
}

// ---------------------------------------------------------------------------
// The guarantees this round added, checked against a real server.
// ---------------------------------------------------------------------------

const GENERATED_TABLE: &str = "OQT_IMPORT_GEN";
/// A table name that only a quote-aware quoter gets right.
const AWKWARD_MYSQL_TABLE: &str = "oqt`tick";
const AWKWARD_ORACLE_TABLE: &str = "oqt lower";

/// A table whose columns the server computes, beside one it does not.
///
/// Oracle allows only ONE identity column per table, so the `BY DEFAULT` case —
/// which IS writable and must stay on offer — gets a table of its own.
fn generated_table_sql(target: Target) -> String {
    if target.is_oracle() {
        format!(
            "CREATE TABLE {GENERATED_TABLE} (\
             ALWAYS_ID NUMBER GENERATED ALWAYS AS IDENTITY, \
             A NUMBER, \
             TOTAL NUMBER GENERATED ALWAYS AS (A * 2) VIRTUAL)"
        )
    } else {
        format!(
            "CREATE TABLE {GENERATED_TABLE} (\
             ALWAYS_ID INT AUTO_INCREMENT PRIMARY KEY, \
             A INT, \
             TOTAL INT AS (A * 2) STORED, \
             VIRT INT AS (A + 1) VIRTUAL)"
        )
    }
}

/// Read a table's ROWS the way `Export Data...` does.
///
/// Through the object-browser readers rather than the editor, because the two
/// carry a NULL differently on purpose: the editor's grid holds the session's
/// NULL display text, and this path holds the driver's own marker — which is
/// what lets a tree export tell a real NULL from a value spelling the same
/// letters. A check about that difference has to read it here.
fn read_table_rows(target: Target, sql: &str) -> Result<QueryResult, String> {
    let info = target.connection_info();
    match target {
        Target::OracleOci => {
            let conn = OracleConnection::connect(
                &info.username,
                &info.password,
                format!("//{}:{}/{}", info.host, info.port, info.service_name),
            )
            .map_err(|e| format!("OCI connect: {e}"))?;
            ObjectBrowser::execute_oci_query(&conn, sql).map_err(|e| e.to_string())
        }
        Target::OracleThin => {
            let mut config = OracleThinConfig::new(
                ConnectTarget::service_name(
                    info.host.clone(),
                    info.port,
                    info.service_name.clone(),
                ),
                info.username.clone(),
                info.password.clone(),
            );
            config.connect_options.disable_oob_probe = true;
            let mut session =
                OracleThinSession::connect(config).map_err(|e| format!("thin connect: {e}"))?;
            ObjectBrowser::execute_thin_query(&mut session, sql)
        }
        Target::MySql | Target::MariaDb => {
            let url = format!(
                "mysql://{}:{}@{}:{}/{}",
                info.username, info.password, info.host, info.port, info.service_name
            );
            let opts = MysqlOpts::from_url(&url).map_err(|e| format!("mysql opts: {e}"))?;
            let mut conn = MysqlConn::new(opts).map_err(|e| format!("mysql connect: {e}"))?;
            space_query::db::query::mysql_executor::MysqlExecutor::execute_for_db_type(
                &mut conn,
                sql,
                info.db_type,
            )
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|result| result.is_select)
            .ok_or_else(|| "the read returned no result set".to_string())
        }
    }
}

/// Read the table's columns through the very function the import dialog uses.
///
/// A raw connection of the right kind, because that read is what changed: the
/// catalog now has to answer "may a statement write into this column?", and the
/// two Oracle drivers must give the same answer.
fn read_table_structure(target: Target, table: &str) -> Result<Vec<TableColumnDetail>, String> {
    let info = target.connection_info();
    match target {
        Target::OracleOci => {
            let conn = OracleConnection::connect(
                &info.username,
                &info.password,
                format!("//{}:{}/{}", info.host, info.port, info.service_name),
            )
            .map_err(|e| format!("OCI connect: {e}"))?;
            ObjectBrowser::get_table_structure(&conn, table).map_err(|e| e.to_string())
        }
        Target::OracleThin => {
            let mut config = OracleThinConfig::new(
                ConnectTarget::service_name(
                    info.host.clone(),
                    info.port,
                    info.service_name.clone(),
                ),
                info.username.clone(),
                info.password.clone(),
            );
            config.connect_options.disable_oob_probe = true;
            let mut session =
                OracleThinSession::connect(config).map_err(|e| format!("thin connect: {e}"))?;
            ObjectBrowser::get_thin_table_structure(&mut session, table)
        }
        Target::MySql | Target::MariaDb => {
            let url = format!(
                "mysql://{}:{}@{}:{}/{}",
                info.username, info.password, info.host, info.port, info.service_name
            );
            let opts = MysqlOpts::from_url(&url).map_err(|e| format!("mysql opts: {e}"))?;
            let mut conn = MysqlConn::new(opts).map_err(|e| format!("mysql connect: {e}"))?;
            MysqlObjectBrowser::get_table_structure_in_schema(
                &mut conn,
                Some(&info.service_name),
                table,
            )
            .map_err(|e| e.to_string())
        }
    }
}

/// (A) A column the server computes is never offered as an import target — and
/// one it merely defaults IS, because an explicit value is legal there.
fn verify_generated_columns(target: Target, h: &mut Harness) -> Result<(), String> {
    let _ = h.run(&target.drop_sql(GENERATED_TABLE));
    h.run(&generated_table_sql(target))
        .map_err(|e| format!("create the generated-column table: {e}"))?;
    let _ = h.run("COMMIT");

    let columns = read_table_structure(target, GENERATED_TABLE)?;
    if columns.is_empty() {
        let _ = h.run(&target.drop_sql(GENERATED_TABLE));
        return Err(format!(
            "the catalog read returned no columns for {GENERATED_TABLE}"
        ));
    }
    let generated: Vec<&str> = columns
        .iter()
        .filter(|column| column.is_generated)
        .map(|column| column.name.as_str())
        .collect();
    let expected: Vec<&str> = if target.is_oracle() {
        vec!["ALWAYS_ID", "TOTAL"]
    } else {
        vec!["TOTAL", "VIRT"]
    };

    println!("  catalog reports generated columns {generated:?}");
    if generated != expected {
        let _ = h.run(&target.drop_sql(GENERATED_TABLE));
        return Err(format!(
            "expected the catalog to report {expected:?} as computed, got {generated:?}"
        ));
    }

    let db_type = target.connection_info().db_type;
    let table_columns = ObjectBrowserWidget::import_targets(db_type, &columns);
    let targets = table_columns.writable();
    let offered: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    let expected_offered: Vec<&str> = if target.is_oracle() {
        vec!["A"]
    } else {
        // `AUTO_INCREMENT` is not computed: an explicit value is legal, so the
        // column has to stay on offer.
        vec!["ALWAYS_ID", "A"]
    };
    if offered != expected_offered {
        let _ = h.run(&target.drop_sql(GENERATED_TABLE));
        return Err(format!(
            "the import should offer {expected_offered:?}, offered {offered:?}"
        ));
    }
    if table_columns.generated_names() != expected {
        let _ = h.run(&target.drop_sql(GENERATED_TABLE));
        return Err("the dialog would not name the columns it left out".into());
    }

    // And the script built from those targets really runs — which is the whole
    // point: mapping the computed column by name is what the server refused.
    let file_columns: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let data = ImportedTable {
        columns: file_columns,
        rows: vec![columns
            .iter()
            .map(|_| Some("7".to_string()))
            .collect::<Vec<ImportCell>>()],
    };
    let mapping = default_mapping(&data.columns, &targets);
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: GENERATED_TABLE,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the import script: {e}"))?;
    h.run_script(&script)
        .map_err(|e| format!("the import into a table with computed columns failed: {e}"))?;
    let _ = h.run("COMMIT");
    let events = h.run(&format!("SELECT COUNT(*) AS N FROM {GENERATED_TABLE}"))?;
    let count = grid_view(&events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();

    // A file with NO header means the TABLE's columns, in the table's order —
    // which is exactly what every data-format export of this table writes. The
    // dialog used to map file position i onto the i-th WRITABLE column, so on
    // Oracle (where the writable column is the SECOND of three) the file's
    // first value — the identity column's — landed in `A`, silently.
    let positional = table_columns.positional_mapping(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let expected = if column.is_generated {
            None
        } else {
            targets.iter().position(|target| target.name == column.name)
        };
        if positional.get(index).copied().flatten() != expected {
            return Err(format!(
                "position {index} ({}) maps to {:?}, expected {expected:?}",
                column.name,
                positional.get(index).copied().flatten()
            ));
        }
    }
    // And the values really land where the file put them.
    let positional_data = ImportedTable {
        columns: (1..=columns.len()).map(|n| format!("COLUMN_{n}")).collect(),
        rows: vec![(1..=columns.len())
            .map(|n| Some((n * 100).to_string()))
            .collect::<Vec<ImportCell>>()],
    };
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: GENERATED_TABLE,
        targets: &targets,
        mapping: &positional,
        data: &positional_data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the headerless import script: {e}"))?;
    h.run_script(&script)
        .map_err(|e| format!("the headerless import failed: {e}"))?;
    let _ = h.run("COMMIT");
    let a_position = columns
        .iter()
        .position(|column| column.name.eq_ignore_ascii_case("A"))
        .ok_or_else(|| "the fixture has no column A".to_string())?;
    let expected_a = ((a_position + 1) * 100).to_string();
    let events = h.run(&format!(
        "SELECT COUNT(*) AS N FROM {GENERATED_TABLE} WHERE A = {expected_a}"
    ))?;
    if single_count(&events).as_deref() != Some("1") {
        let _ = h.run(&target.drop_sql(GENERATED_TABLE));
        return Err(format!(
            "a headerless import put {:?} rows in A = {expected_a}: the file's \
             column {} did not reach A",
            single_count(&events),
            a_position + 1
        ));
    }
    println!("PASS: a headerless file maps by the TABLE's positions, computed columns skipped");

    // The OTHER end of the same rule, while the table is still here. `SQL
    // Inserts` is the one export format whose purpose is to be RUN, so it must
    // not name a column the server computes either — Oracle answers ORA-54013
    // and the MySQL family 3105. This builds the export the way
    // `render_table_export` does and runs what it wrote.
    let generated_names = table_columns.generated_names();
    let all_columns: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let inserts = build_sql_inserts(&GridSqlSelection {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: Some(GENERATED_TABLE.to_string()),
        column_kinds: all_columns.iter().map(|_| SqlValueKind::Number).collect(),
        selected_columns: space_query::ui::grid_sql_export::writable_column_indices(
            &all_columns,
            &generated_names,
        ),
        rows: vec![all_columns.iter().map(|_| Some("9".to_string())).collect()],
        all_columns,
    })
    .into_parts()
    .map_err(|reason| format!("the SQL Inserts export was refused: {reason}"))?
    .0;
    println!(
        "  SQL Inserts export of a table with computed columns:\n    {}",
        inserts.trim()
    );
    let named_a_computed_column = generated_names
        .iter()
        .find(|name| inserts.contains(name.as_str()))
        .cloned();
    let export_ran = h
        .run_script(&inserts)
        .map(|_| ())
        .map_err(|e| e.to_string());
    let _ = h.run("COMMIT");

    let _ = h.run(&target.drop_sql(GENERATED_TABLE));
    let _ = h.run("COMMIT");
    if count.trim() != "1" {
        return Err(format!(
            "the import into a table with computed columns landed {count} rows"
        ));
    }
    println!("PASS: computed columns are left out of the import and the script runs");
    if let Some(name) = named_a_computed_column {
        return Err(format!(
            "the SQL Inserts export named the computed column {name}: {inserts}"
        ));
    }
    export_ran.map_err(|e| {
        format!("the SQL Inserts export of a table with computed columns did not run: {e}")
    })?;
    println!("PASS: a SQL Inserts export of the same table runs on the server");

    // A `GENERATED BY DEFAULT` identity accepts an explicit value, so refusing
    // it would be a new bug of its own.
    if target.is_oracle() {
        let by_default = format!("{GENERATED_TABLE}_D");
        let _ = h.run(&target.drop_sql(&by_default));
        h.run(&format!(
            "CREATE TABLE {by_default} (\
             DEFAULT_ID NUMBER GENERATED BY DEFAULT AS IDENTITY, A NUMBER)"
        ))
        .map_err(|e| format!("create the by-default identity table: {e}"))?;
        let _ = h.run("COMMIT");
        let columns = read_table_structure(target, &by_default)?;
        let offered: Vec<String> = ObjectBrowserWidget::import_targets(db_type, &columns)
            .writable()
            .iter()
            .map(|column| column.name.clone())
            .collect();
        let _ = h.run(&target.drop_sql(&by_default));
        let _ = h.run("COMMIT");
        if offered != ["DEFAULT_ID".to_string(), "A".to_string()] {
            return Err(format!(
                "a BY DEFAULT identity must stay importable; offered {offered:?}"
            ));
        }
        println!("PASS: a BY DEFAULT identity column is still an import target");
    }
    Ok(())
}

/// (A0) A file of LONG values still imports, at the batch size production uses.
///
/// Every other check here runs with `BATCH_ROWS = 2`, on purpose, so the
/// multi-statement path is exercised. That left two things untested, and both
/// were broken — measured, on this fixture, before the fixes:
///
///   * the MySQL family refused the statement outright, `Packet is larger than
///     max_allowed_packet`, because the batch counted ROWS and 100 rows of a
///     200 KB column is twenty megabytes;
///   * Oracle refused the VALUE, `ORA-01704: string literal too long`, because
///     one literal may hold 4000 bytes and no batch size can help that.
///
/// So this is deliberately a file no ordinary fixture looks like: a hundred
/// rows of a document-sized column, imported through the production path at the
/// production batch size.
fn verify_a_production_sized_batch(target: Target, h: &mut Harness) -> Result<(), String> {
    const VALUE_WIDTH: usize = 200_000;
    const HOSTILE_TAIL: &str = "it's & \\ done";
    let table = format!("{SOURCE_TABLE}_BIG");
    let _ = h.run(&target.drop_sql(&table));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {table} (SEQ NUMBER, V CLOB)")
    } else {
        format!("CREATE TABLE {table} (SEQ INT, V LONGTEXT)")
    };
    h.run(&create).map_err(|e| format!("create {table}: {e}"))?;
    let _ = h.run("COMMIT");

    let targets = vec![
        TargetColumn {
            name: "SEQ".to_string(),
            kind: SqlValueKind::Number,
            nullable: false,
        },
        TargetColumn {
            name: "V".to_string(),
            kind: SqlValueKind::String,
            nullable: true,
        },
    ];
    let data = ImportedTable {
        columns: vec!["SEQ".to_string(), "V".to_string()],
        rows: (0..DEFAULT_BATCH_ROWS)
            .map(|row| {
                vec![
                    Some(row.to_string()),
                    // Not just padding: a quote, an ampersand and a backslash
                    // have to survive being cut into pieces.
                    Some(format!("{}{HOSTILE_TAIL}", "x".repeat(VALUE_WIDTH))),
                ]
            })
            .collect(),
    };
    let mapping = default_mapping(&data.columns, &targets);
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: &table,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        // What the object browser actually passes.
        batch_rows: DEFAULT_BATCH_ROWS,
    })
    .map_err(|e| format!("build the production-sized script: {e}"))?;
    let statements = if target.is_oracle() {
        script.matches("INSERT ALL").count()
    } else {
        script.matches("INSERT INTO").count()
    };
    println!(
        "  {DEFAULT_BATCH_ROWS} rows x {VALUE_WIDTH} chars -> {} bytes over {statements} \
         statement(s)",
        script.len()
    );
    if statements < 2 {
        return Err(format!(
            "a {} byte script went out as {statements} statement(s) — the batch is not bounded \
             by size",
            script.len()
        ));
    }

    let ran = h.run_script(&script).map(|_| ()).map_err(|e| e.to_string());
    let _ = h.run("COMMIT");
    let events = h.run(&format!("SELECT COUNT(*) AS N FROM {table}"))?;
    let count = grid_view(&events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    // The length AND the end of the value, because a chunked literal is a place
    // a piece could be dropped or a boundary could eat a character.
    let length_sql = if target.is_oracle() {
        format!(
            "SELECT LENGTH(V) AS L, SUBSTR(V, -{}) AS T FROM {table} WHERE SEQ = 1",
            HOSTILE_TAIL.chars().count()
        )
    } else {
        format!(
            "SELECT CHAR_LENGTH(V) AS L, RIGHT(V, {}) AS T FROM {table} WHERE SEQ = 1",
            HOSTILE_TAIL.chars().count()
        )
    };
    let measured = h.run(&length_sql).unwrap_or_default();
    let (stored, tail) = grid_view(&measured)
        .and_then(|(_, _, rows)| {
            rows.first().map(|row| {
                (
                    row.first().cloned().unwrap_or_default(),
                    row.get(1).cloned().unwrap_or_default(),
                )
            })
        })
        .unwrap_or_default();
    let _ = h.run(&target.drop_sql(&table));
    let _ = h.run("COMMIT");

    ran.map_err(|e| format!("a production-sized batch of long values did not run: {e}"))?;
    if count.trim() != DEFAULT_BATCH_ROWS.to_string() {
        return Err(format!(
            "a production-sized batch landed {count} rows, expected {DEFAULT_BATCH_ROWS}"
        ));
    }
    if stored.trim() != format!("{}", VALUE_WIDTH + HOSTILE_TAIL.chars().count()) {
        return Err(format!(
            "a long value came back {stored} characters long, expected {}",
            VALUE_WIDTH + HOSTILE_TAIL.chars().count()
        ));
    }
    if tail.trim() != HOSTILE_TAIL {
        return Err(format!(
            "the end of a long value came back as {tail:?}, expected {HOSTILE_TAIL:?}"
        ));
    }
    println!(
        "PASS: {DEFAULT_BATCH_ROWS} rows of {VALUE_WIDTH}-character values import whole and intact"
    );
    Ok(())
}

/// (A1) A file's cell cannot add a statement to the import script.
///
/// The Oracle binary literal was built by interpolating the value into
/// `HEXTORAW('<value>')` — the one literal here that neither proved its value
/// nor escaped it — and this writer is what an IMPORT uses, so the value is
/// whatever the file said. A cell holding
/// `41')) SELECT * FROM DUAL; DROP TABLE <victim>; --` closed the call, closed
/// the VALUES list, ended the statement and started one of its own, which the
/// app's own script splitter then handed to the executor. The victim table was
/// really dropped.
///
/// Run on every backend, because "a value is a value" is not an Oracle rule.
/// The script is EXPECTED to fail here — `ORA-01465: invalid hex number` is the
/// honest answer to a non-hex value — and what is checked is that nothing else
/// ran.
fn verify_a_file_cell_cannot_inject_a_statement(
    target: Target,
    h: &mut Harness,
) -> Result<(), String> {
    let victim = format!("{SOURCE_TABLE}_VICTIM");
    let table = format!("{SOURCE_TABLE}_BIN");
    let _ = h.run(&target.drop_sql(&victim));
    let _ = h.run(&target.drop_sql(&table));
    h.run(
        &format!("CREATE TABLE {victim} (X NUMBER)")
            .replace("NUMBER", if target.is_oracle() { "NUMBER" } else { "INT" }),
    )
    .map_err(|e| format!("create the victim table: {e}"))?;
    let binary_type = if target.is_oracle() {
        "RAW(16)"
    } else {
        "VARBINARY(16)"
    };
    let number_type = if target.is_oracle() { "NUMBER" } else { "INT" };
    h.run(&format!(
        "CREATE TABLE {table} (SEQ {number_type}, B {binary_type})"
    ))
    .map_err(|e| format!("create the binary-column table: {e}"))?;
    let _ = h.run("COMMIT");

    let columns = read_table_structure(target, &table)?;
    let db_type = target.connection_info().db_type;
    let targets = ObjectBrowserWidget::import_targets(db_type, &columns).writable();
    let payload = format!("41')) SELECT * FROM DUAL; DROP TABLE {victim}; --");
    let data = ImportedTable {
        columns: vec!["SEQ".to_string(), "B".to_string()],
        rows: vec![vec![Some("1".to_string()), Some(payload.clone())]],
    };
    let mapping = default_mapping(&data.columns, &targets);
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: &table,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the import script: {e}"))?;
    println!("  a hostile cell becomes:\n    {}", script.trim());
    // Run it before judging its shape, so this check measures what the SERVER
    // did and not only what the writer wrote. Whether the statement itself
    // succeeds is not the point — `ORA-01465` is the honest answer to a
    // non-hex value — but nothing ELSE may run.
    let _ = h.run_script(&script);
    let _ = h.run("COMMIT");
    let interpolated = script.contains("HEXTORAW") && !script.contains("HEXTORAW('DEADBEEF')");

    let events = h.run(&target.table_exists_sql(&victim))?;
    let survived = grid_view(&events)
        .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    let _ = h.run(&target.drop_sql(&victim));
    let _ = h.run(&target.drop_sql(&table));
    let _ = h.run("COMMIT");
    if survived.trim() != "1" {
        return Err(
            "BUG: an imported file cell ran a statement of its own — the victim table is gone"
                .to_string(),
        );
    }
    if interpolated {
        return Err(format!(
            "a value that is not provably hex was interpolated into HEXTORAW: {script}"
        ));
    }
    println!("PASS: a hostile file cell stays a VALUE and runs no statement of its own");

    // And a value that IS hex still reaches the server as the same bytes.
    let hex_dialect = SqlWriteDialect::for_connection(&target.connection_info());
    let hex = sql_literal_for_value(hex_dialect, SqlValueKind::Binary, "DEADBEEF")
        .map_err(|reason| format!("a short hex value was refused: {reason:?}"))?;
    let expected = if target.is_oracle() {
        "HEXTORAW('DEADBEEF')"
    } else {
        "'DEADBEEF'"
    };
    if hex != expected {
        return Err(format!(
            "a provably hex value became {hex}, expected {expected}"
        ));
    }
    println!("PASS: a provably hex value still uses the conversion it always did");
    Ok(())
}

/// (A2) The rows `Export Data...` reads must not depend on which driver read
/// them.
///
/// The object browser has its own read — it deliberately skips the grid's ROWID
/// injection — and the thin side of it used to go through the CATALOG value
/// reader instead of the RESULT one. So on thin a SQL NULL arrived as the empty
/// string (OCI sends the driver's own marker), a `RAW` as `[222, 173, 190, 239]`
/// (OCI sends `DEADBEEF`), and a `TIMESTAMP` lost its fractional seconds. Every
/// format then wrote those differences into the file.
///
/// Stated as three absolute properties rather than as a thin-vs-OCI diff,
/// because the harness runs one backend at a time: each driver must satisfy the
/// same three, which is what makes them meet.
fn verify_table_export_read(target: Target) -> Result<(), String> {
    let info = target.connection_info();
    let sql = target.select_sql(SOURCE_TABLE);
    // Both reads on ONE connection, in the production order. A `SQL Inserts`
    // export asks the catalog which columns the server computes and THEN reads
    // the rows, both on the session the action acquired — so the sequence
    // itself is part of what has to work.
    let (columns, result) = match target {
        Target::OracleOci => {
            let conn = OracleConnection::connect(
                &info.username,
                &info.password,
                format!("//{}:{}/{}", info.host, info.port, info.service_name),
            )
            .map_err(|e| format!("OCI connect: {e}"))?;
            let columns = ObjectBrowser::get_table_structure(&conn, SOURCE_TABLE)
                .map_err(|e| format!("OCI structure read: {e}"))?;
            let rows = ObjectBrowser::execute_oci_query(&conn, &sql).map_err(|e| e.to_string())?;
            (columns, rows)
        }
        Target::OracleThin => {
            let mut config = OracleThinConfig::new(
                ConnectTarget::service_name(
                    info.host.clone(),
                    info.port,
                    info.service_name.clone(),
                ),
                info.username.clone(),
                info.password.clone(),
            );
            config.connect_options.disable_oob_probe = true;
            let mut session =
                OracleThinSession::connect(config).map_err(|e| format!("thin connect: {e}"))?;
            let columns = ObjectBrowser::get_thin_table_structure(&mut session, SOURCE_TABLE)
                .map_err(|e| format!("thin structure read: {e}"))?;
            let rows = ObjectBrowser::execute_thin_query(&mut session, &sql)?;
            (columns, rows)
        }
        Target::MySql | Target::MariaDb => {
            let url = format!(
                "mysql://{}:{}@{}:{}/{}",
                info.username, info.password, info.host, info.port, info.service_name
            );
            let opts = MysqlOpts::from_url(&url).map_err(|e| format!("mysql opts: {e}"))?;
            let mut conn = MysqlConn::new(opts).map_err(|e| format!("mysql connect: {e}"))?;
            let columns = MysqlObjectBrowser::get_table_structure_in_schema(
                &mut conn,
                Some(&info.service_name),
                SOURCE_TABLE,
            )
            .map_err(|e| format!("mysql structure read: {e}"))?;
            let rows = space_query::db::query::mysql_executor::MysqlExecutor::execute_for_db_type(
                &mut conn,
                &sql,
                info.db_type,
            )
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|result| result.is_select)
            .ok_or_else(|| "the export read returned no result set".to_string())?;
            (columns, rows)
        }
    };
    if columns.len() != result.columns.len() {
        return Err(format!(
            "the catalog read on the export session saw {} columns, the row read saw {}",
            columns.len(),
            result.columns.len()
        ));
    }
    println!(
        "PASS: the catalog and the rows read on ONE session agree on {} columns",
        columns.len()
    );

    let index_of = |name: &str| -> Option<usize> {
        result
            .columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
    };
    let cell = |row: usize, name: &str| -> String {
        index_of(name)
            .and_then(|index| result.rows.get(row).and_then(|r| r.get(index)))
            .cloned()
            .unwrap_or_default()
    };

    // Row SEQ = 2 is NULL in every column but its key and `CODE`.
    let null_row = result
        .rows
        .iter()
        .position(|row| row.first().map(String::as_str) == Some("2"))
        .ok_or_else(|| "the export read did not return the mostly-NULL row".to_string())?;
    for column in result
        .columns
        .iter()
        .filter(|column| !matches!(column.name.to_ascii_uppercase().as_str(), "SEQ" | "CODE"))
    {
        let value = cell(null_row, &column.name);
        if !QueryCell::is_null_result_text(&value) {
            return Err(format!(
                "{}: a SQL NULL reached the exporter as {value:?}, not as the driver's own \
                 marker — every format would then write it as a value",
                column.name
            ));
        }
    }
    println!("PASS: a SQL NULL reaches the exporter as SQL NULL, not as the empty string");

    let binary_column = if target.is_oracle() { "RAWC" } else { "BINC" };
    let expected_binary = if target.is_oracle() {
        "DEADBEEF"
    } else {
        "abc"
    };
    let binary = cell(0, binary_column);
    if binary != expected_binary {
        return Err(format!(
            "{binary_column} read as {binary:?}, expected {expected_binary:?}"
        ));
    }
    println!("PASS: a binary column reads the same text the grid shows ({binary})");

    let timestamp = cell(0, "TS");
    if !timestamp.contains(".123456") {
        return Err(format!(
            "TS read as {timestamp:?} — the fractional seconds are gone, so the exported \
             value is not the stored one"
        ));
    }
    println!("PASS: a timestamp keeps its fractional seconds ({timestamp})");
    Ok(())
}

/// (B) Values that used to break the `SQL Inserts` file: a trailing backslash,
/// a backslash before a quote, and an `&`.
///
/// Exported to SQL, read back by the importer, loaded into a fresh table, and
/// compared against what the server stored — so a defect anywhere in that chain
/// shows up as a wrong value rather than as a parse error.
fn verify_hostile_sql_round_trip(target: Target, h: &mut Harness) -> Result<(), String> {
    let table = format!("{GENERATED_TABLE}_H");
    let _ = h.run(&target.drop_sql(&table));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {table} (SEQ NUMBER, V VARCHAR2(200))")
    } else {
        format!("CREATE TABLE {table} (SEQ INT, V VARCHAR(200))")
    };
    h.run(&create).map_err(|e| format!("create {table}: {e}"))?;
    let _ = h.run("COMMIT");

    let values = [
        "ends with a backslash \\",
        "before a quote a\\'b",
        "R&D and &&both",
        "windows C:\\path\\",
        // The two escapes MySQL does NOT unescape: inside `LIKE` they are the
        // wildcards, so the backslash is part of the value and the reader has
        // to give it back. A round trip through the server is what says so.
        "like wildcards a\\%b and a\\_b",
        // And the two it DOES: `\Z` is Ctrl-Z and `\b` is a backspace.
        "control a\u{1A}b and a\u{8}c",
    ];
    let dialect = SqlWriteDialect::for_connection(&target.connection_info());
    let grid_rows: Vec<Vec<ImportCell>> = values
        .iter()
        .enumerate()
        .map(|(index, value)| vec![Some((index + 1).to_string()), Some((*value).to_string())])
        .collect();
    let sql_file = build_sql_inserts(&GridSqlSelection {
        dialect,
        table: Some(table.clone()),
        all_columns: vec!["SEQ".to_string(), "V".to_string()],
        column_kinds: vec![SqlValueKind::Number, SqlValueKind::String],
        selected_columns: vec![0, 1],
        rows: grid_rows.clone(),
    })
    .into_parts()
    .map_err(|reason| format!("the long-value SQL Inserts export was refused: {reason}"))?
    .0;

    // (B1) The exported file is READ back with every value intact.
    let parsed = parse(
        &sql_file,
        &ImportOptions {
            format: ExportFormat::SqlInserts,
            has_header: true,
            null_text: NULL_TEXT.to_string(),
        },
    )
    .map_err(|e| format!("the exported SQL did not parse: {e}"))?;
    if parsed.rows != grid_rows {
        let _ = h.run(&target.drop_sql(&table));
        return Err(format!(
            "the exported SQL read back differently\n  wrote {grid_rows:?}\n  read  {:?}",
            parsed.rows
        ));
    }

    // (B2) …and RUNS, on the same session the user would run it on, storing the
    // same text. On Oracle that also proves the `&` never became a prompt.
    h.run_script(&sql_file)
        .map_err(|e| format!("the exported SQL failed to run: {e}"))?;
    let _ = h.run("COMMIT");
    let events = h.run(&format!("SELECT SEQ, V FROM {table} ORDER BY SEQ"))?;
    let stored: Vec<String> = grid_view(&events)
        .map(|(_, _, rows)| {
            rows.iter()
                .map(|row| row.get(1).cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let _ = h.run(&target.drop_sql(&table));
    let _ = h.run("COMMIT");
    if stored != values.iter().map(|v| (*v).to_string()).collect::<Vec<_>>() {
        return Err(format!(
            "the server stored {stored:?}, the grid held {values:?}"
        ));
    }
    println!("PASS: backslashes and ampersands survive the SQL export, the import and the server");
    Ok(())
}

/// (C) A table whose name needs quoting is named correctly by the import
/// script, using the same qualifier the object browser produces.
fn verify_awkward_table_name(target: Target, h: &mut Harness) -> Result<(), String> {
    let (raw_name, quoted) = if target.is_oracle() {
        (
            AWKWARD_ORACLE_TABLE.to_string(),
            format!("\"{AWKWARD_ORACLE_TABLE}\""),
        )
    } else {
        (
            AWKWARD_MYSQL_TABLE.to_string(),
            format!("`{}`", AWKWARD_MYSQL_TABLE.replace('`', "``")),
        )
    };
    let _ = h.run(&format!("DROP TABLE {quoted}"));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {quoted} (A NUMBER)")
    } else {
        format!("CREATE TABLE {quoted} (A INT)")
    };
    h.run(&create)
        .map_err(|e| format!("create {quoted}: {e}"))?;
    let _ = h.run("COMMIT");

    // Exactly what `Import Data...` passes: the browser's own qualified name.
    let qualified = ObjectBrowserWidget::qualified_object_name(
        target.connection_info().db_type,
        None,
        &raw_name,
    );
    let targets = vec![TargetColumn {
        name: "A".to_string(),
        kind: SqlValueKind::Number,
        nullable: true,
    }];
    let data = ImportedTable {
        columns: vec!["A".to_string()],
        rows: vec![vec![Some("1".to_string())]],
    };
    let mapping = default_mapping(&data.columns, &targets);
    let script = build_insert_script(&ImportRequest {
        dialect: SqlWriteDialect::for_connection(&target.connection_info()),
        table: &qualified,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the import script: {e}"))?;
    println!("  qualified name {qualified:?} -> {}", script.trim());
    let ran = h.run_script(&script);
    let _ = h.run("COMMIT");
    let count = if ran.is_ok() {
        let events = h.run(&format!("SELECT COUNT(*) AS N FROM {quoted}"))?;
        grid_view(&events)
            .and_then(|(_, _, rows)| rows.first().and_then(|row| row.last().cloned()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let _ = h.run(&format!("DROP TABLE {quoted}"));
    let _ = h.run("COMMIT");
    ran.map_err(|e| format!("the import named the table wrongly: {e}"))?;
    if count.trim() != "1" {
        return Err(format!("the import landed {count:?} rows, expected 1"));
    }
    println!("PASS: a table name that needs quoting is named correctly by the import");
    Ok(())
}

/// (E) The batched script this app writes for an import is itself a `.sql` file
/// this app can import — every row of it.
///
/// One statement carries MANY rows: a MySQL-family batch is one `INSERT` with a
/// multi-row `VALUES` list, an Oracle batch is one `INSERT ALL` with many
/// `INTO`s. The SQL reader used to take the first row of each and drop the rest
/// in silence, so a five-row script came back as one row per statement. Checked
/// against a real server, because the rows have to land as well as parse.
fn verify_batched_script_reimport(target: Target, h: &mut Harness) -> Result<(), String> {
    let table = format!("{GENERATED_TABLE}_B");
    let _ = h.run(&target.drop_sql(&table));
    let create = if target.is_oracle() {
        format!("CREATE TABLE {table} (SEQ NUMBER, V VARCHAR2(50))")
    } else {
        format!("CREATE TABLE {table} (SEQ INT, V VARCHAR(50))")
    };
    h.run(&create).map_err(|e| format!("create {table}: {e}"))?;
    let _ = h.run("COMMIT");

    let dialect = SqlWriteDialect::for_connection(&target.connection_info());
    let targets = vec![
        TargetColumn {
            name: "SEQ".to_string(),
            kind: SqlValueKind::Number,
            nullable: true,
        },
        TargetColumn {
            name: "V".to_string(),
            kind: SqlValueKind::String,
            nullable: true,
        },
    ];
    let rows = 5usize;
    let data = ImportedTable {
        columns: vec!["SEQ".to_string(), "V".to_string()],
        rows: (1..=rows)
            .map(|index| vec![Some(index.to_string()), Some(format!("row {index}"))])
            .collect(),
    };
    let mapping = default_mapping(&data.columns, &targets);
    // BATCH_ROWS is 2, so five rows are three statements and every one of them
    // carries more than one row.
    let script = build_insert_script(&ImportRequest {
        dialect,
        table: &table,
        targets: &targets,
        mapping: &mapping,
        data: &data,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the batched script: {e}"))?;

    // Read the script back the way `Import Data...` reads a `.sql` file.
    let parsed = parse(
        &script,
        &ImportOptions {
            format: ExportFormat::SqlInserts,
            has_header: true,
            null_text: NULL_TEXT.to_string(),
        },
    )
    .map_err(|e| format!("the batched script did not parse: {e}"))?;
    if parsed.rows != data.rows {
        let _ = h.run(&target.drop_sql(&table));
        return Err(format!(
            "the batched script read back as {} of {rows} rows: {:?}",
            parsed.rows.len(),
            parsed.rows
        ));
    }

    // And those rows really load.
    let reimport = build_insert_script(&ImportRequest {
        dialect,
        table: &table,
        targets: &targets,
        mapping: &default_mapping(&parsed.columns, &targets),
        data: &parsed,
        batch_rows: BATCH_ROWS,
    })
    .map_err(|e| format!("build the re-import script: {e}"))?;
    h.run_script(&reimport)
        .map_err(|e| format!("the re-import failed: {e}"))?;
    let _ = h.run("COMMIT");
    let events = h.run(&format!("SELECT COUNT(*) AS N FROM {table}"))?;
    let count = grid_view(&events)
        .and_then(|(_, _, row)| row.first().and_then(|row| row.last().cloned()))
        .unwrap_or_default();
    let _ = h.run(&target.drop_sql(&table));
    let _ = h.run("COMMIT");
    if count.trim() != rows.to_string() {
        return Err(format!(
            "the re-imported batched script landed {count} rows, expected {rows}"
        ));
    }
    println!("PASS: every row of a batched import script survives being re-imported");
    Ok(())
}

/// (D) A MySQL-family session whose own `sql_mode` turns backslash escaping OFF
/// must not have its backslashes doubled.
///
/// `sql_mode` is a per-connection setting of this app
/// (`ConnectionAdvancedSettings::mysql_sql_mode`, sent at connect), so this is
/// reachable, not hypothetical. Both spellings are run against the same server:
/// the family default stores TWO characters where the connection's own rules
/// store one, which is the defect, and the writer now asks the connection.
fn verify_no_backslash_escapes(target: Target, h: &mut Harness) -> Result<(), String> {
    if target.is_oracle() {
        return Ok(());
    }
    let table = format!("{GENERATED_TABLE}_NBE");
    let _ = h.run(&target.drop_sql(&table));
    h.run(&format!("CREATE TABLE {table} (SEQ INT, V VARCHAR(50))"))
        .map_err(|e| format!("create {table}: {e}"))?;
    h.run("SET SESSION sql_mode = 'TRADITIONAL,NO_BACKSLASH_ESCAPES'")
        .map_err(|e| format!("set NO_BACKSLASH_ESCAPES: {e}"))?;
    let _ = h.run("COMMIT");

    let mut info = target.connection_info();
    info.advanced.mysql_sql_mode = "TRADITIONAL,NO_BACKSLASH_ESCAPES".to_string();
    let targets = vec![
        TargetColumn {
            name: "SEQ".to_string(),
            kind: SqlValueKind::Number,
            nullable: true,
        },
        TargetColumn {
            name: "V".to_string(),
            kind: SqlValueKind::String,
            nullable: true,
        },
    ];
    let script_for = |dialect, seq: &str| {
        let data = ImportedTable {
            columns: vec!["SEQ".to_string(), "V".to_string()],
            rows: vec![vec![Some(seq.to_string()), Some("a\\b".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        build_insert_script(&ImportRequest {
            dialect,
            table: &table,
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: BATCH_ROWS,
        })
    };

    // 1: the family default — what the writer used to assume for everyone.
    h.run_script(
        &script_for(SqlWriteDialect::family_default(info.db_type), "1")
            .map_err(|e| format!("build the family-default script: {e}"))?,
    )
    .map_err(|e| format!("family-default import: {e}"))?;
    // 2: the rules this connection actually runs under.
    h.run_script(
        &script_for(SqlWriteDialect::for_connection(&info), "2")
            .map_err(|e| format!("build the connection script: {e}"))?,
    )
    .map_err(|e| format!("connection-dialect import: {e}"))?;
    let _ = h.run("COMMIT");

    let events = h.run(&format!("SELECT SEQ, V FROM {table} ORDER BY SEQ"))?;
    let stored: Vec<String> = grid_view(&events)
        .map(|(_, _, rows)| {
            rows.iter()
                .map(|row| row.get(1).cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let _ = h.run("SET SESSION sql_mode = 'TRADITIONAL'");
    let _ = h.run(&target.drop_sql(&table));
    let _ = h.run("COMMIT");

    if stored.len() != 2 {
        return Err(format!("expected two rows back, got {stored:?}"));
    }
    if stored[0] != "a\\\\b" {
        return Err(format!(
            "the family default should have stored a DOUBLED backslash under \
             NO_BACKSLASH_ESCAPES (that is the defect); it stored {:?}",
            stored[0]
        ));
    }
    if stored[1] != "a\\b" {
        return Err(format!(
            "the connection's own rules must store one backslash; stored {:?}",
            stored[1]
        ));
    }
    println!("PASS: NO_BACKSLASH_ESCAPES is honoured (default doubled it, the connection did not)");
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
