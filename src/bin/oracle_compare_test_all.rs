use chrono::NaiveDateTime;
use fltk::{app, input::IntInput};
use serde::{Deserialize, Serialize};
use space_query::db::{ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tns_thin::exec::{
    OracleColumnType as ThinOracleColumnType, OracleValue as ThinOracleValue,
    StatementRequest as ThinStatementRequest,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GridSnapshot {
    sql: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DriverRunSnapshot {
    failures: Vec<String>,
    grids: Vec<GridSnapshot>,
    open_cursor_count: u32,
}

const CURRENT_SID_SQL: &str = "select sys_context('USERENV', 'SID') from dual";

fn oracle_connection_info(mode: OracleDriverMode) -> ConnectionInfo {
    let host = env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("ORACLE_TEST_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(1521);
    let service = env::var("ORACLE_TEST_SERVICE_NAME")
        .or_else(|_| env::var("ORACLE_TEST_SERVICE"))
        .unwrap_or_else(|_| "FREE".to_string());
    let username = env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".to_string());
    let password = env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let mut info = ConnectionInfo::new_with_type(
        mode.label(),
        &username,
        &password,
        &host,
        port,
        &service,
        DatabaseType::Oracle,
    );
    info.advanced.oracle_driver_mode = mode;
    info
}

fn run_script(mode: OracleDriverMode, sql: &str) -> Result<(Vec<QueryProgress>, u32), String> {
    let mut connection = DatabaseConnection::new();
    connection.connect(oracle_connection_info(mode))?;
    connection.set_auto_commit(true)?;
    let sid = read_current_sid(&connection, mode)?;
    let shared_connection = Arc::new(Mutex::new(connection));
    let timeout_input = IntInput::default();
    let mut widget = SqlEditorWidget::new(shared_connection, timeout_input);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let trace_progress = env::var_os("ORACLE_COMPARE_TRACE_PROGRESS").is_some();
    {
        let progress = Arc::clone(&progress);
        let done = Arc::clone(&done);
        widget.set_progress_callback(move |event| {
            if trace_progress {
                trace_progress_event(mode, &event);
            }
            if matches!(progress_inner(&event), QueryProgress::BatchFinished) {
                done.store(true, Ordering::SeqCst);
            }
            progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });
    }

    let timeout_secs = env::var("ORACLE_COMPARE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(300);
    let repeat_count = env::var("ORACLE_COMPARE_REPEAT_SAME_SESSION")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    for _ in 0..repeat_count {
        done.store(false, Ordering::SeqCst);
        widget.execute_script_for_harness(sql);
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
            if !app::wait() {
                app::check();
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if !done.load(Ordering::SeqCst) {
            return Err(format!(
                "Oracle {} script execution timed out after {timeout_secs}s",
                mode.label(),
            ));
        }
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
    let open_cursor_count = read_open_cursor_count_for_sid(mode, &sid)?;
    Ok((events, open_cursor_count))
}

fn trace_progress_event(mode: OracleDriverMode, event: &QueryProgress) {
    match progress_inner(event) {
        QueryProgress::BatchStart { activity } => {
            eprintln!("[{}] batch start: {activity}", mode.label());
        }
        QueryProgress::StatementStart { index, .. } => {
            eprintln!("[{}] statement start #{index}", mode.label());
        }
        QueryProgress::SelectStart { index, columns, .. } => {
            eprintln!(
                "[{}] select start #{index}, columns={}",
                mode.label(),
                columns.len()
            );
        }
        QueryProgress::Rows { index, rows } => {
            eprintln!("[{}] rows #{index}, count={}", mode.label(), rows.len());
        }
        QueryProgress::StatementFinished { index, result, .. } => {
            eprintln!(
                "[{}] statement finished #{index}, success={}, sql={}",
                mode.label(),
                result.success,
                result.sql.lines().next().unwrap_or_default().trim()
            );
        }
        QueryProgress::BatchFinished => {
            eprintln!("[{}] batch finished", mode.label());
        }
        QueryProgress::WorkerPanicked { message } => {
            eprintln!("[{}] worker panicked: {message}", mode.label());
        }
        _ => {}
    }
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. } => progress_inner(progress),
        other => other,
    }
}

fn read_current_sid(
    connection: &DatabaseConnection,
    mode: OracleDriverMode,
) -> Result<String, String> {
    match mode {
        OracleDriverMode::Oci => {
            let conn = connection
                .get_connection()
                .ok_or_else(|| "Oracle OCI connection is not available".to_string())?;
            conn.query_row_as::<String>(CURRENT_SID_SQL, &[])
                .map(|value| value.trim().to_string())
                .map_err(|err| format!("failed to read Oracle OCI SID: {err}"))
        }
        OracleDriverMode::Thin => {
            let session = connection
                .get_oracle_thin_connection()
                .ok_or_else(|| "Oracle Thin connection is not available".to_string())?;
            let mut session = session
                .lock()
                .map_err(|_| "Oracle Thin session mutex poisoned".to_string())?;
            let result = session
                .execute_typed_fetch_all(
                    &ThinStatementRequest::query(CURRENT_SID_SQL, 1),
                    &[ThinOracleColumnType::Varchar],
                )
                .map_err(|err| format!("failed to read Oracle Thin SID: {err}"))?;
            result
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(thin_value_to_text)
                .map(|value| value.trim().to_string())
                .ok_or_else(|| "Oracle Thin SID query returned no value".to_string())
        }
    }
}

fn read_open_cursor_count_for_sid(mode: OracleDriverMode, sid: &str) -> Result<u32, String> {
    let sid = sid
        .trim()
        .parse::<u32>()
        .map_err(|err| format!("invalid Oracle SID `{sid}`: {err}"))?;
    let sql = format!(
        "select s.value \
         from v$sesstat s \
         join v$statname n on n.statistic# = s.statistic# \
         where s.sid = {sid} and n.name = 'opened cursors current'"
    );
    let mut monitor = DatabaseConnection::new();
    monitor.connect(oracle_connection_info(mode))?;

    match mode {
        OracleDriverMode::Oci => {
            let conn = monitor
                .get_connection()
                .ok_or_else(|| "Oracle OCI monitor connection is not available".to_string())?;
            let value = conn
                .query_row_as::<i64>(&sql, &[])
                .map_err(|err| format!("failed to read Oracle OCI open cursor count: {err}"))?;
            u32::try_from(value).map_err(|err| {
                format!("Oracle OCI open cursor count {value} cannot fit in u32: {err}")
            })
        }
        OracleDriverMode::Thin => {
            let session = monitor
                .get_oracle_thin_connection()
                .ok_or_else(|| "Oracle Thin monitor connection is not available".to_string())?;
            let mut session = session
                .lock()
                .map_err(|_| "Oracle Thin monitor session mutex poisoned".to_string())?;
            let result = session
                .execute_typed_fetch_all(
                    &ThinStatementRequest::query(sql, 1),
                    &[ThinOracleColumnType::Number],
                )
                .map_err(|err| format!("failed to read Oracle Thin open cursor count: {err}"))?;
            result
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(thin_value_to_text)
                .ok_or_else(|| "Oracle Thin open cursor count query returned no value".to_string())?
                .trim()
                .parse::<u32>()
                .map_err(|err| format!("invalid Oracle Thin open cursor count: {err}"))
        }
    }
}

fn thin_value_to_text(value: &ThinOracleValue) -> Option<&str> {
    match value {
        ThinOracleValue::Text(value) | ThinOracleValue::Number(value) => Some(value),
        _ => None,
    }
}

fn failures(progress: &[QueryProgress]) -> Vec<String> {
    progress
        .iter()
        .filter_map(|event| match progress_inner(event) {
            QueryProgress::StatementFinished { index, result, .. } if !result.success => Some(
                format!("#{index}: {} => {}", result.sql.trim(), result.message),
            ),
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

fn parse_datetime_cell(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
        .ok()
}

fn cells_match_for_compare(column_name: &str, oci: &str, thin: &str) -> bool {
    if oci == thin {
        return true;
    }
    if column_name.eq_ignore_ascii_case("ROWID") {
        return !oci.trim().is_empty() && !thin.trim().is_empty();
    }
    let Some(oci_dt) = parse_datetime_cell(oci) else {
        return false;
    };
    let Some(thin_dt) = parse_datetime_cell(thin) else {
        return false;
    };
    (oci_dt - thin_dt).num_seconds().abs() <= 600
}

fn grid_cells_match_for_compare(oci: &GridSnapshot, thin: &GridSnapshot) -> bool {
    if oci.columns != thin.columns || oci.rows.len() != thin.rows.len() {
        return false;
    }
    oci.rows
        .iter()
        .zip(thin.rows.iter())
        .all(|(oci_row, thin_row)| {
            oci_row.len() == thin_row.len()
                && oci_row.iter().zip(thin_row.iter()).enumerate().all(
                    |(index, (oci_cell, thin_cell))| {
                        let column_name = oci
                            .columns
                            .get(index)
                            .map(String::as_str)
                            .unwrap_or_default();
                        cells_match_for_compare(column_name, oci_cell, thin_cell)
                    },
                )
        })
}

fn cleanup_sql() -> &'static str {
    "BEGIN
  FOR r IN (
    SELECT object_name, object_type
    FROM user_objects
    WHERE object_name LIKE 'OQT_%'
       OR object_name LIKE 'QT_%'
       OR object_name LIKE 'FMT_%'
  ) LOOP
    BEGIN
      IF r.object_type = 'TABLE' THEN
        EXECUTE IMMEDIATE 'DROP TABLE ' || r.object_name || ' CASCADE CONSTRAINTS PURGE';
      ELSIF r.object_type = 'VIEW' THEN
        EXECUTE IMMEDIATE 'DROP VIEW ' || r.object_name;
      ELSIF r.object_type = 'PACKAGE' THEN
        EXECUTE IMMEDIATE 'DROP PACKAGE ' || r.object_name;
      ELSIF r.object_type = 'SYNONYM' THEN
        EXECUTE IMMEDIATE 'DROP SYNONYM ' || r.object_name;
      ELSIF r.object_type = 'TRIGGER' THEN
        EXECUTE IMMEDIATE 'DROP TRIGGER ' || r.object_name;
      ELSIF r.object_type = 'SEQUENCE' THEN
        EXECUTE IMMEDIATE 'DROP SEQUENCE ' || r.object_name;
      ELSIF r.object_type = 'TYPE' THEN
        EXECUTE IMMEDIATE 'DROP TYPE ' || r.object_name || ' FORCE';
      ELSIF r.object_type = 'FUNCTION' THEN
        EXECUTE IMMEDIATE 'DROP FUNCTION ' || r.object_name;
      ELSIF r.object_type = 'PROCEDURE' THEN
        EXECUTE IMMEDIATE 'DROP PROCEDURE ' || r.object_name;
      END IF;
    EXCEPTION
      WHEN OTHERS THEN NULL;
    END;
  END LOOP;
END;
/
"
}

fn mode_from_arg(value: &str) -> Result<OracleDriverMode, String> {
    match value.to_ascii_lowercase().as_str() {
        "oci" => Ok(OracleDriverMode::Oci),
        "thin" => Ok(OracleDriverMode::Thin),
        other => Err(format!("unknown Oracle driver mode `{other}`")),
    }
}

fn child_run(mode: OracleDriverMode, path: &str) -> Result<DriverRunSnapshot, String> {
    let _app = app::App::default();
    let sql = if path == "__cleanup__" {
        cleanup_sql().to_string()
    } else {
        let body =
            std::fs::read_to_string(path).map_err(|err| format!("failed to read {path}: {err}"))?;
        let body = if thin_protocol_314() {
            oracle_314_compare_script_body_for_path(Path::new(path), &body)?
        } else {
            body
        };
        format!("BEGIN DBMS_RANDOM.SEED(424242); END;\n/\n{body}\n")
    };

    let (progress, open_cursor_count) = run_script(mode, &sql)?;
    Ok(DriverRunSnapshot {
        failures: failures(&progress),
        grids: grids(&progress),
        open_cursor_count,
    })
}

fn run_child(mode: OracleDriverMode, path: &str) -> Result<DriverRunSnapshot, String> {
    let mode_arg = match mode {
        OracleDriverMode::Oci => "oci",
        OracleDriverMode::Thin => "thin",
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_nanos();
    let output_path = env::temp_dir().join(format!(
        "oracle_compare_{}_{}_{}.json",
        std::process::id(),
        mode_arg,
        nonce
    ));
    let status = Command::new(env::current_exe().map_err(|err| err.to_string())?)
        .arg("--child")
        .arg(mode_arg)
        .arg(path)
        .arg(&output_path)
        .stdout(Stdio::null())
        .status()
        .map_err(|err| format!("failed to spawn {} child: {err}", mode.label()))?;
    if !status.success() {
        return Err(format!(
            "Oracle {} child failed with status {}",
            mode.label(),
            status
        ));
    }

    let output = std::fs::read(&output_path).map_err(|err| {
        format!(
            "failed to read {} child output {}: {err}",
            mode.label(),
            output_path.display()
        )
    })?;
    let _ = std::fs::remove_file(&output_path);
    serde_json::from_slice(&output)
        .map_err(|err| format!("failed to parse {} child output: {err}", mode.label()))
}

fn parent_run(path: &str) -> Result<(), String> {
    let thin_cleanup = run_child(OracleDriverMode::Thin, "__cleanup__")?;
    if !thin_cleanup.failures.is_empty() {
        return Err(format!(
            "Oracle Thin cleanup failures:\n{}",
            thin_cleanup.failures.join("\n")
        ));
    }
    let thin = run_child(OracleDriverMode::Thin, path)?;

    let oci_cleanup = run_child(OracleDriverMode::Oci, "__cleanup__")?;
    if !oci_cleanup.failures.is_empty() {
        return Err(format!(
            "Oracle OCI cleanup failures:\n{}",
            oci_cleanup.failures.join("\n")
        ));
    }
    let oci = run_child(OracleDriverMode::Oci, path)?;

    if !oci.failures.is_empty() || !thin.failures.is_empty() {
        return Err(format!(
            "OCI failures:\n{}\nThin failures:\n{}",
            oci.failures.join("\n"),
            thin.failures.join("\n")
        ));
    }

    println!(
        "Oracle open cursors after {path}: OCI={}, Thin={}",
        oci.open_cursor_count, thin.open_cursor_count
    );
    if thin.open_cursor_count > oci.open_cursor_count {
        return Err(format!(
            "Oracle Thin open cursor count ({}) is greater than OCI ({}) after {path}",
            thin.open_cursor_count, oci.open_cursor_count
        ));
    }

    let same_cells = oci.grids.len() == thin.grids.len()
        && oci
            .grids
            .iter()
            .zip(thin.grids.iter())
            .all(|(oci_grid, thin_grid)| grid_cells_match_for_compare(oci_grid, thin_grid));
    if !same_cells {
        eprintln!(
            "Oracle Thin select cells differ from OCI: OCI grids={}, Thin grids={}",
            oci.grids.len(),
            thin.grids.len()
        );
        for (index, (oci_grid, thin_grid)) in oci.grids.iter().zip(thin.grids.iter()).enumerate() {
            if !grid_cells_match_for_compare(oci_grid, thin_grid) {
                eprintln!("first mismatch at select grid #{index}");
                eprintln!("OCI: {oci_grid:#?}");
                eprintln!("Thin: {thin_grid:#?}");
                break;
            }
        }
        if oci.grids.len() != thin.grids.len() {
            eprintln!("select grid count mismatch");
        }
        return Err("Oracle Thin select cells differ from OCI".to_string());
    }

    println!(
        "Oracle OCI and Thin matched {} select/grid result(s) from {path}",
        thin.grids.len()
    );
    Ok(())
}

fn thin_protocol_314() -> bool {
    env::var("ORACLE_THIN_DESIRED_PROTOCOL")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        == Some(314)
}

fn oracle_314_compare_script_body_for_path(path: &Path, body: &str) -> Result<String, String> {
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let expanded = expand_sql_includes_for_compare(body, base_dir, &mut Vec::new())?;
    Ok(oracle_314_compare_script_body(&expanded))
}

fn oracle_314_compare_script_body(body: &str) -> String {
    let mut body = body.to_string();
    body = oracle_314_skip_implicit_call_block(&body);
    disable_serveroutput_on_lines(&body)
}

fn expand_sql_includes_for_compare(
    sql_text: &str,
    base_dir: &Path,
    include_stack: &mut Vec<PathBuf>,
) -> Result<String, String> {
    let mut expanded = String::with_capacity(sql_text.len());
    for line in sql_text.lines() {
        let Some((raw_path, relative_to_current_file)) = parse_sql_include_line(line) else {
            expanded.push_str(line);
            expanded.push('\n');
            continue;
        };
        let include_path = resolve_sql_include_path(raw_path, base_dir, relative_to_current_file);
        let canonical = include_path.canonicalize().map_err(|err| {
            format!(
                "failed to resolve include {}: {err}",
                include_path.display()
            )
        })?;
        if include_stack.contains(&canonical) {
            return Err(format!(
                "recursive SQL include detected at {}",
                canonical.display()
            ));
        }
        include_stack.push(canonical.clone());
        let include_body = std::fs::read_to_string(&canonical)
            .map_err(|err| format!("failed to read include {}: {err}", canonical.display()))?;
        let include_base = canonical.parent().unwrap_or_else(|| Path::new("."));
        let include_body =
            expand_sql_includes_for_compare(&include_body, include_base, include_stack)?;
        include_stack.pop();
        expanded.push_str(&include_body);
        if !include_body.ends_with('\n') {
            expanded.push('\n');
        }
    }
    Ok(expanded)
}

fn parse_sql_include_line(line: &str) -> Option<(&str, bool)> {
    let trimmed = line.trim_start();
    let relative_to_current_file = trimmed.starts_with("@@");
    let rest = if relative_to_current_file {
        trimmed.strip_prefix("@@")?
    } else {
        trimmed.strip_prefix('@')?
    };
    let rest = rest.split("--").next().unwrap_or(rest).trim();
    if rest.is_empty() {
        return None;
    }
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim();
    if rest.is_empty() {
        None
    } else {
        Some((rest, relative_to_current_file))
    }
}

fn resolve_sql_include_path(
    raw_path: &str,
    base_dir: &Path,
    relative_to_current_file: bool,
) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if relative_to_current_file {
        return base_dir.join(path);
    }
    if path.exists() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn oracle_314_skip_implicit_call_block(sql_text: &str) -> String {
    sql_text.replace(
        "BEGIN\n  oqt_pkg.p_implicit(10);\nEND;\n/",
        "BEGIN\n  DBMS_OUTPUT.PUT_LINE('[p_implicit] skipped: implicit results unsupported by protocol 314');\nEND;\n/",
    )
}

fn disable_serveroutput_on_lines(sql_text: &str) -> String {
    sql_text
        .lines()
        .map(|line| {
            if line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("set serveroutput on")
            {
                "SET SERVEROUTPUT OFF;".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("--child") {
        if args.len() != 3 && args.len() != 4 {
            eprintln!("usage: oracle_compare_test_all --child <oci|thin> <path> [out-json]");
            std::process::exit(2);
        }
        let mode = match mode_from_arg(&args[1]) {
            Ok(mode) => mode,
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(2);
            }
        };
        match child_run(mode, &args[2]) {
            Ok(snapshot) => {
                let json = match serde_json::to_string(&snapshot) {
                    Ok(json) => json,
                    Err(err) => {
                        eprintln!("failed to serialize child output: {err}");
                        std::process::exit(1);
                    }
                };
                if let Some(path) = args.get(3) {
                    if let Err(err) = std::fs::write(path, json) {
                        eprintln!("failed to write child output {path}: {err}");
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
        return;
    }

    let path = args
        .pop()
        .unwrap_or_else(|| "test/test_all.sql".to_string());
    if let Err(err) = parent_run(&path) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_314_compare_body_expands_includes_before_legacy_rewrite() {
        let dir = env::temp_dir().join(format!(
            "oracle_compare_include_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp include dir");
        let include = dir.join("include.sql");
        std::fs::write(
            &include,
            "SET SERVEROUTPUT ON\nBEGIN\n  oqt_pkg.p_implicit(10);\nEND;\n/\n",
        )
        .expect("write include");
        let main = dir.join("main.sql");
        let body = format!("@{};\n", include.display());

        let rewritten =
            oracle_314_compare_script_body_for_path(&main, &body).expect("rewrite compare script");

        assert!(!rewritten.contains('@'));
        assert!(rewritten.contains("SET SERVEROUTPUT OFF;"));
        assert!(rewritten.contains("implicit results unsupported by protocol 314"));
        assert!(!rewritten.contains("SET SERVEROUTPUT ON"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sql_include_expansion_rejects_recursive_include() {
        let dir = env::temp_dir().join(format!(
            "oracle_compare_include_cycle_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp include dir");
        let include = dir.join("cycle.sql");
        std::fs::write(&include, "@@cycle.sql;\n").expect("write recursive include");

        let err = oracle_314_compare_script_body_for_path(&include, "@@cycle.sql;\n")
            .expect_err("recursive include should be rejected");

        assert!(err.contains("recursive SQL include"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
