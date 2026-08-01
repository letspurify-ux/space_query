#![allow(clippy::cargo, clippy::pedantic)]

use fltk::{app, input::IntInput};
use serde::{Deserialize, Serialize};
use space_query::db::{ConnectionInfo, DatabaseConnection, DatabaseType};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use space_query::utils::SqlCommaListLayout;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize)]
struct GridSnapshot {
    sql: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunSnapshot {
    failures: Vec<String>,
    grids: Vec<GridSnapshot>,
}

fn database_type(value: &str) -> Result<DatabaseType, String> {
    match value.to_ascii_lowercase().as_str() {
        "mysql" => Ok(DatabaseType::MySQL),
        "mariadb" => Ok(DatabaseType::MariaDB),
        other => Err(format!("unknown MySQL-family database type `{other}`")),
    }
}

fn comma_list_layout(value: &str) -> Result<SqlCommaListLayout, String> {
    match value.to_ascii_lowercase().as_str() {
        "wrapped" => Ok(SqlCommaListLayout::Wrapped),
        "stacked" => Ok(SqlCommaListLayout::Stacked),
        other => Err(format!("unknown SQL comma-list layout `{other}`")),
    }
}

fn connection_info(db_type: DatabaseType) -> ConnectionInfo {
    let host = env::var("SPACE_QUERY_TEST_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("SPACE_QUERY_TEST_MYSQL_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3306);
    let database = env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
        .unwrap_or_else(|_| "query_tool_test".to_string());
    let username = env::var("SPACE_QUERY_TEST_MYSQL_USER").unwrap_or_else(|_| "root".to_string());
    let password =
        env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD").unwrap_or_else(|_| "password".to_string());
    ConnectionInfo::new_with_type(
        "MYSQL_FIXTURE_SNAPSHOT",
        &username,
        &password,
        &host,
        port,
        &database,
        db_type,
    )
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. } => progress_inner(progress),
        other => other,
    }
}

fn run_script(db_type: DatabaseType, sql: &str) -> Result<Vec<QueryProgress>, String> {
    let mut connection = DatabaseConnection::new();
    connection.set_auto_commit(true)?;
    connection.connect(connection_info(db_type))?;
    let shared_connection = Arc::new(Mutex::new(connection));
    let timeout_input = IntInput::default();
    let mut widget = SqlEditorWidget::new(Arc::clone(&shared_connection), timeout_input);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    {
        let progress = Arc::clone(&progress);
        let done = Arc::clone(&done);
        widget.set_progress_callback(move |event| {
            if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                done.store(true, Ordering::SeqCst);
            }
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });
    }

    widget.execute_script_for_harness(sql);
    let timeout_secs = env::var("MYSQL_FIXTURE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(600);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
        if !app::wait() {
            app::check();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if !done.load(Ordering::SeqCst) {
        return Err(format!(
            "{db_type:?} script execution timed out after {timeout_secs}s"
        ));
    }
    let drain_deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < drain_deadline {
        if !app::wait() {
            app::check();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let events = progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Ok(events)
}

fn failures(progress: &[QueryProgress]) -> Vec<String> {
    progress
        .iter()
        .filter_map(|event| match progress_inner(event) {
            QueryProgress::StatementFinished { index, result, .. } if !result.success => Some(
                format!("#{index}: {} => {}", result.sql.trim(), result.message),
            ),
            QueryProgress::WorkerPanicked { message } => {
                Some(format!("worker panicked: {message}"))
            }
            _ => None,
        })
        .collect()
}

fn grids(progress: &[QueryProgress]) -> Vec<GridSnapshot> {
    let mut columns_by_index = HashMap::<usize, Vec<String>>::new();
    let mut rows_by_index = HashMap::<usize, Vec<Vec<String>>>::new();
    let mut snapshots = Vec::new();
    for event in progress {
        match progress_inner(event) {
            QueryProgress::SelectStart { index, columns, .. } => {
                columns_by_index.insert(*index, columns.clone());
            }
            QueryProgress::Rows { index, rows } => {
                rows_by_index
                    .entry(*index)
                    .or_default()
                    .extend(rows.iter().cloned());
            }
            QueryProgress::StatementFinished { index, result, .. }
                if result.success && result.is_select =>
            {
                let columns = columns_by_index.remove(index).unwrap_or_else(|| {
                    result
                        .columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect()
                });
                let rows = rows_by_index
                    .remove(index)
                    .unwrap_or_else(|| result.rows.clone());
                snapshots.push(GridSnapshot {
                    sql: result.sql.trim().to_string(),
                    columns,
                    rows,
                });
            }
            _ => {}
        }
    }
    snapshots
}

fn snapshot(
    db_type: DatabaseType,
    path: &Path,
    format_layout: Option<SqlCommaListLayout>,
) -> Result<RunSnapshot, String> {
    let mut sql = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    if let Some(layout) = format_layout {
        sql = SqlEditorWidget::format_script_for_harness(&sql, db_type, layout);
    }
    let progress = run_script(db_type, &sql)?;
    Ok(RunSnapshot {
        failures: failures(&progress),
        grids: grids(&progress),
    })
}

fn main() {
    let _app = app::App::default();
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2 || args.len() > 5 {
        eprintln!(
            "usage: mysql_fixture_snapshot <mysql|mariadb> <path> \
             [out-json] [--format <wrapped|stacked>]"
        );
        std::process::exit(2);
    }
    let db_type = match database_type(&args[0]) {
        Ok(db_type) => db_type,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    let mut output_path = None;
    let mut format_layout = None;
    let mut index = 2;
    while index < args.len() {
        if args[index] == "--format" {
            let Some(value) = args.get(index + 1) else {
                eprintln!("--format requires <wrapped|stacked>");
                std::process::exit(2);
            };
            format_layout = match comma_list_layout(value) {
                Ok(layout) => Some(layout),
                Err(err) => {
                    eprintln!("{err}");
                    std::process::exit(2);
                }
            };
            index += 2;
        } else if output_path.is_none() {
            output_path = Some(args[index].as_str());
            index += 1;
        } else {
            eprintln!("unexpected argument `{}`", args[index]);
            std::process::exit(2);
        }
    }
    match snapshot(db_type, Path::new(&args[1]), format_layout) {
        Ok(snapshot) => {
            let json = match serde_json::to_string(&snapshot) {
                Ok(json) => json,
                Err(err) => {
                    eprintln!("failed to serialize snapshot: {err}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = output_path {
                if let Err(err) = std::fs::write(path, json) {
                    eprintln!("failed to write snapshot {path}: {err}");
                    std::process::exit(1);
                }
            } else {
                println!("{json}");
            }
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
