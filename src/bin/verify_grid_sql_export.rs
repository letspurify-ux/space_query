#![allow(clippy::cargo, clippy::pedantic)]

// UI-path verification for the result-grid SQL export menu items:
//   SQL Inserts, SQL Updates, Where Clause.
//
// Drives the real `ResultTableWidget` (the same widget the GUI uses) on the
// process main thread, sets a real selection, then generates SQL through the
// same snapshot + builder functions the popup menu uses and puts it on the OS
// clipboard, reading it back with `pbpaste`. That proves the text a user would
// actually paste, including the per-driver literal rules.
//
// The app itself reaches these through
// `MainWindow::copy_result_selection_as_sql`, which additionally resolves the
// base table and (for SQL Updates) queries the primary key; those two steps need
// a live connection and are covered by the per-database live checks.
//
// Usage: cargo run --bin verify_grid_sql_export

use fltk::{app, prelude::*, window::Window};
use space_query::db::{ColumnInfo, DatabaseType, QueryResult, SqlValueKind};
use space_query::ui::grid_sql_export::{build_sql_inserts, build_sql_updates, build_where_clause};
use space_query::ui::ResultTableWidget;
use std::process::Command;
use std::time::Duration;

fn column(name: &str, data_type: &str, kind: SqlValueKind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: data_type.into(),
        kind,
    }
}

/// One column per `SqlValueKind`, with values rendered the way the Oracle and
/// MySQL executors render them.
fn sample_result() -> QueryResult {
    QueryResult {
        sql: "SELECT ID, NAME, HIREDATE, RAW_COL, FLAG, NOTE FROM HR.EMP".into(),
        columns: vec![
            column("ID", "Number", SqlValueKind::Number),
            column("NAME", "Varchar", SqlValueKind::String),
            column("HIREDATE", "Date", SqlValueKind::Temporal),
            column("RAW_COL", "Raw", SqlValueKind::Binary),
            column("FLAG", "Varchar", SqlValueKind::String),
            column("NOTE", "Clob", SqlValueKind::Unknown),
        ],
        rows: vec![
            vec![
                "7369".into(),
                "it's SMITH".into(),
                "1980-12-17 00:00:00".into(),
                "DEADBEEF".into(),
                "00123".into(),
                "[LOB]".into(),
            ],
            vec![
                "7499".into(),
                "ALLEN".into(),
                "NULL".into(),
                "0A0B".into(),
                "007".into(),
                "plain".into(),
            ],
        ],
        row_count: 2,
        execution_time: Duration::from_millis(1),
        message: String::new(),
        is_select: true,
        success: true,
    }
}

fn read_clipboard() -> Result<String, String> {
    Command::new("pbpaste")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
        .map_err(|err| format!("pbpaste should run on macOS: {err}"))
}

/// Copy `sql` the way `MainWindow::finish_sql_clipboard_copy` does, read it back
/// off the OS clipboard, and compare against `expected`.
fn check_clipboard(label: &str, sql: &str, expected: &str, failures: &mut Vec<String>) {
    app::copy(sql);
    app::wait_for(0.2).ok();
    match read_clipboard() {
        Ok(clip) => {
            println!("--- {label} ---");
            println!("{clip}");
            if clip == expected {
                println!("PASS: {label} matches the expected SQL");
            } else {
                failures.push(format!(
                    "{label} mismatch\n  expected: {expected:?}\n  actual:   {clip:?}"
                ));
            }
        }
        Err(err) => failures.push(err),
    }
}

fn main() {
    let app = app::App::default();
    let mut win = Window::new(0, 0, 700, 400, "verify_grid_sql_export");
    let mut grid = ResultTableWidget::new();
    win.end();
    win.show();

    grid.display_result(&sample_result());
    // Let FLTK process the widget/layout events the display triggers.
    app::wait_for(0.2).ok();

    let mut failures: Vec<String> = Vec::new();

    // (1) Oracle, whole grid selected: every kind in one statement per row.
    grid.select_all();
    app::wait_for(0.1).ok();
    let Some(oracle) = grid.sql_export_selection(DatabaseType::Oracle, Some("HR.EMP".into()))
    else {
        eprintln!("FAILURES:\n  - selection snapshot was empty after select_all");
        std::process::exit(1);
    };

    check_clipboard(
        "Oracle SQL Inserts",
        &build_sql_inserts(&oracle),
        concat!(
            "INSERT INTO HR.EMP (ID, NAME, HIREDATE, RAW_COL, FLAG, NOTE) VALUES ",
            "(7369, 'it''s SMITH', TO_DATE('1980-12-17 00:00:00','YYYY-MM-DD HH24:MI:SS'), ",
            "HEXTORAW('DEADBEEF'), '00123', '[LOB]');\n",
            "INSERT INTO HR.EMP (ID, NAME, HIREDATE, RAW_COL, FLAG, NOTE) VALUES ",
            "(7499, 'ALLEN', NULL, HEXTORAW('0A0B'), '007', 'plain');\n",
        ),
        &mut failures,
    );

    check_clipboard(
        "Oracle SQL Updates (PK = ID)",
        &build_sql_updates(&oracle, &["ID".to_string()]),
        concat!(
            "UPDATE HR.EMP SET NAME = 'it''s SMITH', ",
            "HIREDATE = TO_DATE('1980-12-17 00:00:00','YYYY-MM-DD HH24:MI:SS'), ",
            "RAW_COL = HEXTORAW('DEADBEEF'), FLAG = '00123', NOTE = '[LOB]' WHERE ID = 7369;\n",
            "UPDATE HR.EMP SET NAME = 'ALLEN', HIREDATE = NULL, RAW_COL = HEXTORAW('0A0B'), ",
            "FLAG = '007', NOTE = 'plain' WHERE ID = 7499;\n",
        ),
        &mut failures,
    );

    check_clipboard(
        "Oracle SQL Updates (no PK)",
        &build_sql_updates(&oracle, &[]),
        concat!(
            "UPDATE HR.EMP SET ID = 7369, NAME = 'it''s SMITH', ",
            "HIREDATE = TO_DATE('1980-12-17 00:00:00','YYYY-MM-DD HH24:MI:SS'), ",
            "RAW_COL = HEXTORAW('DEADBEEF'), FLAG = '00123', NOTE = '[LOB]';\n",
            "UPDATE HR.EMP SET ID = 7499, NAME = 'ALLEN', HIREDATE = NULL, ",
            "RAW_COL = HEXTORAW('0A0B'), FLAG = '007', NOTE = 'plain';\n",
        ),
        &mut failures,
    );

    // (2) MySQL: backticked identifiers, quoted ISO datetimes, lossy binary.
    let Some(mysql) = grid.sql_export_selection(DatabaseType::MySQL, Some("hr.emp".into())) else {
        failures.push("MySQL selection snapshot was empty".into());
        finish(&failures, win, app);
        return;
    };
    check_clipboard(
        "MySQL SQL Inserts",
        &build_sql_inserts(&mysql),
        concat!(
            "INSERT INTO `hr`.`emp` (`ID`, `NAME`, `HIREDATE`, `RAW_COL`, `FLAG`, `NOTE`) VALUES ",
            "(7369, 'it''s SMITH', '1980-12-17 00:00:00', 'DEADBEEF', '00123', '[LOB]');\n",
            "INSERT INTO `hr`.`emp` (`ID`, `NAME`, `HIREDATE`, `RAW_COL`, `FLAG`, `NOTE`) VALUES ",
            "(7499, 'ALLEN', NULL, '0A0B', '007', 'plain');\n",
        ),
        &mut failures,
    );

    // (3) Where Clause over a single column: collapses into IN.
    grid.get_widget().set_selection(0, 0, 1, 0);
    app::wait_for(0.1).ok();
    match grid.sql_export_selection(DatabaseType::Oracle, Some("HR.EMP".into())) {
        Some(one_column) => check_clipboard(
            "Oracle Where Clause (one column)",
            &build_where_clause(&one_column),
            "ID IN (7369, 7499)",
            &mut failures,
        ),
        None => failures.push("single-column selection snapshot was empty".into()),
    }

    // (4) Where Clause over several columns: AND inside a row, OR between rows,
    //     and a NULL compared with IS NULL.
    grid.get_widget().set_selection(0, 0, 1, 2);
    app::wait_for(0.1).ok();
    match grid.sql_export_selection(DatabaseType::Oracle, Some("HR.EMP".into())) {
        Some(three_columns) => check_clipboard(
            "Oracle Where Clause (three columns, two rows)",
            &build_where_clause(&three_columns),
            concat!(
                "(ID = 7369 AND NAME = 'it''s SMITH' AND ",
                "HIREDATE = TO_DATE('1980-12-17 00:00:00','YYYY-MM-DD HH24:MI:SS')) OR ",
                "(ID = 7499 AND NAME = 'ALLEN' AND HIREDATE IS NULL)",
            ),
            &mut failures,
        ),
        None => failures.push("three-column selection snapshot was empty".into()),
    }

    finish(&failures, win, app);
}

fn finish(failures: &[String], mut win: Window, app: app::App) {
    win.hide();
    app::wait_for(0.0).ok();
    let _ = app;

    println!();
    if failures.is_empty() {
        println!("ALL CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for failure in failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}
