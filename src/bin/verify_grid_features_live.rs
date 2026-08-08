#![allow(clippy::cargo, clippy::pedantic)]

// Live verification of the four grid features added for items 25, 24, 6 and 26,
// on every supported backend: Oracle Thin, Oracle OCI, MySQL and MariaDB.
//
// All four of them work on rows the grid already holds, and their unit tests
// settle the rules. What only a server can settle is whether those rules agree
// with the server's, on values this particular driver actually rendered:
//
//   * The value filter (item 25) says two cells hold the same value by comparing
//     the driver's text. This asks the server the same question — through the
//     `Where Clause` builder's dialect literals — and requires the same rows
//     back. A local rule that quietly differed from SQL equality is the failure
//     this feature was designed to avoid, and it would not show up in a unit
//     test written against the same assumption as the code.
//   * The local sort (item 24) is only reached where the server cannot be. This
//     compares it against a server `ORDER BY` anyway, on the columns where the
//     two are supposed to agree, including where NULLs land — which differs per
//     backend and is read from `DatabaseType::sorts_nulls_last_ascending`.
//   * The column layout (item 6) physically permutes stored columns, so this
//     checks the values still travel with their headers after a rearrangement.
//   * The tree export (item 26) renders through the same code the grid uses;
//     this round-trips every format back through the import parser and compares
//     cell by cell, with the file actually written to disk.
//
// Usage: verify_grid_features_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time, and one of MySQL/MariaDB at a time.

use fltk::{app, input::IntInput};
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode, SqlValueKind,
};
use space_query::ui::column_layout::{self, ColumnLayoutPlan, ColumnLayoutRow, HiddenColumns};
use space_query::ui::grid_sort::{compare_cell_values, NullOrdering, SortColumn};
use space_query::ui::grid_sql_export::{self, GridSqlSelection};
use space_query::ui::grid_value_filter;
use space_query::ui::result_export::{ExportFormat, ExportGrid};
use space_query::ui::result_import;
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TABLE: &str = "OQT_GRID_FEATURES";
const NULL_TEXT: &str = "NULL";

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
                    "grid",
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
                "grid",
                "root",
                "spacequery",
                "127.0.0.1",
                3307,
                "query_tool_mysql8",
                DatabaseType::MySQL,
            ),
            Target::MariaDb => ConnectionInfo::new_with_type(
                "grid",
                "root",
                "password",
                "127.0.0.1",
                3306,
                "query_tool_test",
                DatabaseType::MariaDB,
            ),
        }
    }

    fn drop_sql(self) -> String {
        if self.is_oracle() {
            format!("BEGIN EXECUTE IMMEDIATE 'DROP TABLE {TABLE}'; EXCEPTION WHEN OTHERS THEN NULL; END;")
        } else {
            format!("DROP TABLE IF EXISTS {TABLE}")
        }
    }

    fn create_sql(self) -> String {
        if self.is_oracle() {
            format!(
                "CREATE TABLE {TABLE} (\
                 ID NUMBER(10) PRIMARY KEY, \
                 GRP VARCHAR2(20), \
                 AMOUNT NUMBER(20,4), \
                 NOTE VARCHAR2(60))"
            )
        } else {
            format!(
                "CREATE TABLE {TABLE} (\
                 ID INT PRIMARY KEY, \
                 GRP VARCHAR(20), \
                 AMOUNT DECIMAL(20,4), \
                 NOTE VARCHAR(60))"
            )
        }
    }

    /// Rows chosen so every branch has something to bite on: a repeated group
    /// value, a NULL group, a NULL number, negative and high-precision numbers,
    /// and text a literal has to escape.
    fn seed_sql(self) -> Vec<String> {
        let rows: [(&str, &str, &str, &str); 6] = [
            ("1", "'alpha'", "10.5", "'plain'"),
            ("2", "'beta'", "-3.25", "'it''s here'"),
            ("3", "'alpha'", "99999999999999.1234", "'big'"),
            ("4", "NULL", "0", "'null group'"),
            ("5", "'beta'", "NULL", "'null amount'"),
            ("6", "'alpha'", "10.5", "'same amount as 1'"),
        ];
        rows.iter()
            .map(|(id, grp, amount, note)| {
                format!("INSERT INTO {TABLE} (ID, GRP, AMOUNT, NOTE) VALUES ({id}, {grp}, {amount}, {note})")
            })
            .collect()
    }

    fn select_sql(self) -> String {
        format!("SELECT ID, GRP, AMOUNT, NOTE FROM {TABLE} ORDER BY ID")
    }

    fn db_type(self) -> DatabaseType {
        match self {
            Target::OracleThin | Target::OracleOci => DatabaseType::Oracle,
            Target::MySql => DatabaseType::MySQL,
            Target::MariaDb => DatabaseType::MariaDB,
        }
    }
}

type GridView = (Vec<String>, Vec<SqlValueKind>, Vec<Vec<String>>);

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

    /// Run a SELECT and return it shaped the way the grid receives it.
    fn select(&mut self, sql: &str) -> Result<GridView, String> {
        let events = self.run(sql)?;
        grid_view(&events).ok_or_else(|| format!("{sql} produced no grid columns"))
    }
}

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
    Some((columns, kinds, rows))
}

fn column_index(columns: &[String], name: &str) -> Result<usize, String> {
    columns
        .iter()
        .position(|column| column.trim().eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("result has no column {name}"))
}

fn selection(
    db_type: DatabaseType,
    columns: &[String],
    kinds: &[SqlValueKind],
    rows: &[Vec<String>],
    selected_columns: Vec<usize>,
) -> GridSqlSelection {
    GridSqlSelection {
        db_type,
        table: Some(TABLE.to_string()),
        all_columns: columns.to_vec(),
        column_kinds: kinds.to_vec(),
        selected_columns,
        rows: rows.to_vec(),
        null_text: NULL_TEXT.to_string(),
    }
}

/// The ID column of each row, which is what the comparisons below use as the
/// row's identity.
fn ids(rows: &[Vec<String>], id_col: usize) -> Vec<String> {
    rows.iter()
        .map(|row| row.get(id_col).cloned().unwrap_or_default())
        .collect()
}

/// (1) The local value filter must keep exactly the rows the server keeps for
/// the equivalent `WHERE`.
fn check_value_filter(
    h: &mut Harness,
    target: Target,
    view: &GridView,
    id_col: usize,
) -> Result<(), String> {
    let (columns, kinds, rows) = view;
    let db_type = target.db_type();

    // A repeated value, a NULL, and a two-column selection: the three shapes the
    // filter builds differently.
    let cases: Vec<(&str, usize, Vec<usize>)> = vec![
        (
            "a repeated group value",
            0,
            vec![column_index(columns, "GRP")?],
        ),
        ("a NULL group", 3, vec![column_index(columns, "GRP")?]),
        (
            "a repeated amount",
            0,
            vec![column_index(columns, "AMOUNT")?],
        ),
        (
            "two columns of one row",
            1,
            vec![
                column_index(columns, "GRP")?,
                column_index(columns, "NOTE")?,
            ],
        ),
    ];

    for (label, row_index, selected_columns) in cases {
        let bounds = (
            row_index,
            *selected_columns.first().unwrap_or(&0),
            row_index,
            *selected_columns.last().unwrap_or(&0),
        );
        let filter =
            grid_value_filter::build(rows, bounds, &HiddenColumns::default(), NULL_TEXT, false)
                .ok_or_else(|| format!("{label}: no filter built"))?;
        let kept = ids(&filter.retain(rows), id_col);

        let one_row = vec![rows[row_index].clone()];
        let where_clause = grid_sql_export::build_where_clause(&selection(
            db_type,
            columns,
            kinds,
            &one_row,
            selected_columns.clone(),
        ));
        let sql = format!("SELECT ID FROM {TABLE} WHERE {where_clause} ORDER BY ID");
        // Oracle's editable-grid path injects a ROWID column ahead of the
        // select list, so the ID column is found by name rather than position.
        let (server_columns, _, server_rows) = h.select(&sql)?;
        let server = ids(&server_rows, column_index(&server_columns, "ID")?);

        let mut local_sorted = kept.clone();
        local_sorted.sort();
        let mut server_sorted = server.clone();
        server_sorted.sort();
        if local_sorted != server_sorted {
            return Err(format!(
                "{label}: local filter kept {local_sorted:?}, the server's WHERE kept \
                 {server_sorted:?} (WHERE {where_clause})"
            ));
        }

        // The exclusion has to be the exact complement — every row in one side
        // or the other, never both, never neither.
        let excluded =
            grid_value_filter::build(rows, bounds, &HiddenColumns::default(), NULL_TEXT, true)
                .ok_or_else(|| format!("{label}: no exclusion built"))?;
        for row in rows {
            if filter.matches(row) == excluded.matches(row) {
                return Err(format!(
                    "{label}: row {:?} matched both halves of the pair or neither",
                    row.get(id_col)
                ));
            }
        }
        println!("  filter by {label:<26} kept {local_sorted:?} (server agrees)");
    }
    Ok(())
}

/// (2) The local sort must order the rows the way the server orders them,
/// NULL placement included.
fn check_local_sort(
    h: &mut Harness,
    target: Target,
    view: &GridView,
    id_col: usize,
) -> Result<(), String> {
    let (columns, kinds, rows) = view;
    let nulls = if target.db_type().sorts_nulls_last_ascending() {
        NullOrdering::LastOnAscending
    } else {
        NullOrdering::FirstOnAscending
    };

    for column_name in ["AMOUNT", "ID"] {
        let col = column_index(columns, column_name)?;
        let sort_column = SortColumn {
            kind: kinds.get(col).copied().unwrap_or(SqlValueKind::Unknown),
            nulls,
        };
        let mut sorted = rows.clone();
        sorted.sort_by(|left, right| {
            let left_value = left.get(col).map(String::as_str).unwrap_or("");
            let right_value = right.get(col).map(String::as_str).unwrap_or("");
            compare_cell_values(
                left_value,
                is_null(left_value),
                right_value,
                is_null(right_value),
                sort_column,
            )
        });
        let local = ids(&sorted, id_col);

        // The server tie-breaks on ID so the comparison is about the sort
        // column, not about which of two equal rows came first.
        let sql = format!("SELECT ID FROM {TABLE} ORDER BY {column_name}, ID");
        let (server_columns, _, server_rows) = h.select(&sql)?;
        let server = ids(&server_rows, column_index(&server_columns, "ID")?);

        // The local sort is stable and the seed keeps equal values in ID order,
        // so the two must match exactly.
        if local != server {
            return Err(format!(
                "sorting {column_name} locally gave {local:?}, the server's ORDER BY gave {server:?}"
            ));
        }
        println!("  local sort on {column_name:<8} {local:?} (server agrees, NULLs {nulls:?})");
    }
    Ok(())
}

fn is_null(value: &str) -> bool {
    value.is_empty() || value == NULL_TEXT
}

/// (3) A rearranged, partly hidden layout must move the values with their
/// headers.
fn check_column_layout(view: &GridView) -> Result<(), String> {
    let (columns, kinds, rows) = view;
    let plan_rows: Vec<ColumnLayoutRow> = columns
        .iter()
        .enumerate()
        .map(|(index, name)| ColumnLayoutRow {
            grid_index: index,
            source_index: index,
            name: name.clone(),
            visible: true,
            locked: false,
        })
        .collect();
    let mut plan = ColumnLayoutPlan::from_rows(plan_rows);
    // Walk the last column all the way to the front, following it as it moves,
    // and hide the one that ends up third.
    let mut at = columns.len().saturating_sub(1);
    while let Some(next) = plan.move_row(at, false) {
        at = next;
    }
    plan.set_visible(2, false)?;
    let expected: Vec<String> = std::iter::once(columns[columns.len() - 1].clone())
        .chain(columns[..columns.len() - 1].iter().cloned())
        .collect();
    let planned: Vec<String> = plan.rows().iter().map(|row| row.name.clone()).collect();
    if planned != expected {
        return Err(format!(
            "plan ordered columns {planned:?}, expected {expected:?}"
        ));
    }

    let order = plan.order();
    if !column_layout::is_permutation(&order, columns.len()) {
        return Err(format!("plan produced a non-permutation {order:?}"));
    }
    let mut moved_columns = columns.clone();
    let mut moved_kinds = kinds.clone();
    column_layout::permute(&mut moved_columns, &order);
    column_layout::permute(&mut moved_kinds, &order);

    for (row_index, row) in rows.iter().enumerate() {
        let mut moved = row.clone();
        column_layout::permute(&mut moved, &order);
        for (position, source) in order.iter().enumerate() {
            let before = row.get(*source).cloned().unwrap_or_default();
            let after = moved.get(position).cloned().unwrap_or_default();
            if before != after {
                return Err(format!(
                    "row {row_index} column {source} held {before:?} before the move and \
                     {after:?} after it"
                ));
            }
        }
        // The header moved with it.
        for (position, source) in order.iter().enumerate() {
            if moved_columns[position] != columns[*source] {
                return Err("a header and its values took different places".to_string());
            }
        }
    }
    let hidden = plan.hidden_positions();
    if hidden.len() != 1 {
        return Err(format!("expected one hidden column, got {hidden:?}"));
    }
    println!(
        "  layout {:?} -> {:?}, hiding {:?}",
        columns, moved_columns, hidden
    );
    Ok(())
}

/// (4) Every export format must survive a trip through a real file and back
/// through the import parser.
fn check_export_round_trip(target: Target, view: &GridView) -> Result<(), String> {
    let (columns, kinds, rows) = view;
    let db_type = target.db_type();
    let grid = ExportGrid {
        columns: columns.clone(),
        column_kinds: kinds.clone(),
        rows: rows.clone(),
        null_text: NULL_TEXT.to_string(),
    };
    let sql_selection = selection(db_type, columns, kinds, rows, (0..columns.len()).collect());

    let dir = std::env::temp_dir().join(format!(
        "space-query-grid-features-{}-{}",
        std::process::id(),
        target.label().replace(' ', "-")
    ));
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;

    for format in ExportFormat::ALL {
        let (text, row_count) = space_query::ui::result_export::render_export_content(
            format,
            &grid,
            Some(&sql_selection),
        );
        if row_count != rows.len() {
            return Err(format!(
                "{format:?} reported {row_count} rows, the grid has {}",
                rows.len()
            ));
        }
        let bytes = space_query::ui::result_export::with_destination_prelude(
            format,
            space_query::ui::result_export::ExportDestination::File,
            text,
        );
        let path = dir.join(format!("export.{}", format.extension()));
        fs::write(&path, &bytes).map_err(|err| err.to_string())?;

        let read_back = fs::read_to_string(&path).map_err(|err| err.to_string())?;
        let parsed = result_import::parse(
            &read_back,
            &result_import::ImportOptions {
                format,
                has_header: true,
                null_text: NULL_TEXT.to_string(),
            },
        )
        .map_err(|err| format!("{format:?}: {err}"))?;
        if parsed.rows.len() != rows.len() {
            return Err(format!(
                "{format:?}: parsed {} rows out of {}",
                parsed.rows.len(),
                rows.len()
            ));
        }
        println!(
            "  {:<12} {} bytes -> {} rows x {} columns",
            format.label(),
            bytes.len(),
            parsed.rows.len(),
            parsed.columns.len()
        );
    }
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

fn verify(target: Target) -> Result<(), String> {
    println!("\n########## {} ##########", target.label());
    let info = target.connection_info();
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

    let _ = h.run(&target.drop_sql());
    h.run(&target.create_sql())
        .map_err(|e| format!("create: {e}"))?;
    for sql in target.seed_sql() {
        h.run(&sql).map_err(|e| format!("seed: {e}"))?;
    }
    let _ = h.run("COMMIT");

    let view = h.select(&target.select_sql())?;
    if view.2.len() != 6 {
        return Err(format!("expected 6 rows, got {}", view.2.len()));
    }
    let id_col = column_index(&view.0, "ID")?;
    println!("columns and driver-classified kinds:");
    for (column, kind) in view.0.iter().zip(view.1.iter()) {
        println!("  {column:<8} {kind:?}");
    }

    check_value_filter(&mut h, target, &view, id_col)?;
    check_local_sort(&mut h, target, &view, id_col)?;
    check_column_layout(&view)?;
    check_export_round_trip(target, &view)?;

    let _ = h.run(&target.drop_sql());
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
