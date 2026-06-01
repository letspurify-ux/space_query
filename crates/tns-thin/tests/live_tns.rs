use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tns_thin::exec::{OracleValue, StatementRequest};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

static OBJECT_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn negotiated_protocol_matches_requested_protocol() {
    let conn = connect();
    if let Some(requested) = protocol_env("ORACLE_THIN_DESIRED_PROTOCOL") {
        assert_eq!(conn.capabilities().protocol_version, Some(requested));
    } else {
        assert!(
            conn.capabilities().protocol_version.is_some(),
            "thin connection should report the negotiated protocol"
        );
    }
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn fetch_and_fetch_all_return_all_rows() {
    let mut conn = connect();
    let sql = "SELECT level AS n, 'R' || TO_CHAR(level) AS label FROM dual CONNECT BY level <= 7";
    let request = StatementRequest::query(sql, 2);

    let initial = conn
        .query_described_initial_request(&request)
        .expect("initial described fetch");
    assert_eq!(
        initial
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["N", "LABEL"]
    );
    assert_eq!(rows_to_strings(&initial.result.rows), expected_rows(1, 2));
    assert!(
        !initial.result.exhausted,
        "initial fetch should leave rows for explicit fetch calls"
    );
    let cursor_id = initial
        .result
        .cursor_id
        .expect("initial fetch should leave an open cursor");

    let fetched = conn
        .fetch_ref_cursor_batch(cursor_id, &initial.columns, 2, false)
        .expect("fetch next batch");
    assert_eq!(rows_to_strings(&fetched.rows), expected_rows(3, 4));
    assert!(
        !fetched.exhausted,
        "second fetch should still leave rows for fetch_all"
    );

    let remaining = conn
        .fetch_ref_cursor_all(cursor_id, initial.columns.clone(), 3)
        .expect("fetch all remaining rows");
    assert!(remaining.result.exhausted);
    assert_eq!(rows_to_strings(&remaining.result.rows), expected_rows(5, 7));
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn commit_and_auto_commit_make_changes_visible() {
    let config = live_config();
    let table = unique_table_name("TX");
    let _guard = TableDropGuard::new(config.clone(), table.clone());
    let mut writer = connect_with_config(config.clone());
    let mut reader = connect_with_config(config);

    writer
        .query_drop(&format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY)"))
        .expect("create transaction table");

    writer
        .query_drop(&format!("INSERT INTO {table} VALUES (1)"))
        .expect("insert before explicit commit");
    assert_eq!(
        select_count(&mut reader, &table),
        0,
        "uncommitted row must not be visible to another session"
    );
    writer.commit().expect("commit transaction");
    assert_eq!(
        select_count(&mut reader, &table),
        1,
        "committed row must be visible to another session"
    );

    let mut request = StatementRequest::statement(format!("INSERT INTO {table} VALUES (2)"));
    request.auto_commit = true;
    writer
        .execute_typed(&request, &[])
        .expect("insert with protocol auto-commit");
    writer
        .rollback()
        .expect("rollback after auto-commit should not undo committed row");
    assert_eq!(
        select_count(&mut reader, &table),
        2,
        "auto-committed row must survive a later rollback"
    );
}

#[test]
#[ignore = "requires local Oracle listener via ORACLE_THIN_TEST_* environment variables"]
fn cancel_interrupts_long_running_query() {
    let mut conn = connect();
    conn.set_call_timeout(Some(Duration::from_secs(10)))
        .expect("set cancel test timeout");
    let cancel = conn.cancel_handle();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        cancel.break_execution()
    });

    let started = Instant::now();
    let result = conn.query(
        "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c",
        1,
    );
    cancel_thread
        .join()
        .expect("cancel thread should not panic")
        .expect("send thin cancel marker");

    let message = result
        .expect_err("cancelled query should fail with ORA-01013")
        .to_string()
        .to_ascii_lowercase();
    assert!(
        message.contains("ora-01013") || message.contains("user requested cancel"),
        "expected ORA-01013 cancel error, got {message}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel took {:?}",
        started.elapsed()
    );
}

fn live_config() -> OracleThinConfig {
    let host = env_or("ORACLE_THIN_TEST_HOST", "ORACLE_TEST_HOST", "127.0.0.1");
    let port = env_or("ORACLE_THIN_TEST_PORT", "ORACLE_TEST_PORT", "1521")
        .parse::<u16>()
        .expect("invalid Oracle test port");
    let service = env_or("ORACLE_THIN_TEST_SERVICE", "ORACLE_TEST_SERVICE", "FREE");
    let username = env_or(
        "ORACLE_THIN_TEST_USERNAME",
        "ORACLE_TEST_USERNAME",
        "system",
    );
    let password = env_or(
        "ORACLE_THIN_TEST_PASSWORD",
        "ORACLE_TEST_PASSWORD",
        "password",
    );
    let mut config = OracleThinConfig::new(
        ConnectTarget::service_name(host, port, service),
        username,
        password,
    );
    if let Some(version) = protocol_env("ORACLE_THIN_DESIRED_PROTOCOL") {
        config.connect_options.desired_protocol_version = version;
        config.connect_options.minimum_protocol_version = version;
    }
    if let Some(version) = protocol_env("ORACLE_THIN_MINIMUM_PROTOCOL") {
        config.connect_options.minimum_protocol_version = version;
    }
    if let Some(version) = ttc_field_version_env("ORACLE_THIN_TTC_FIELD_VERSION") {
        config.connect_options.desired_ttc_field_version = Some(version);
    }
    config.connect_options.disable_oob_probe = false;
    config
}

fn connect() -> OracleThinSession {
    connect_with_config(live_config())
}

fn connect_with_config(config: OracleThinConfig) -> OracleThinSession {
    OracleThinSession::connect(config).expect("thin login")
}

fn env_or(primary: &str, fallback: &str, default: &str) -> String {
    std::env::var(primary)
        .or_else(|_| std::env::var(fallback))
        .unwrap_or_else(|_| default.to_string())
}

fn protocol_env(name: &str) -> Option<u16> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .parse::<u16>()
            .unwrap_or_else(|err| panic!("invalid {name} value `{trimmed}`: {err}")),
    )
}

fn ttc_field_version_env(name: &str) -> Option<u8> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .parse::<u8>()
            .unwrap_or_else(|err| panic!("invalid {name} value `{trimmed}`: {err}")),
    )
}

fn unique_table_name(prefix: &str) -> String {
    let counter = OBJECT_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "OQT_{}_{}_{}",
        prefix,
        std::process::id() % 100_000,
        counter
    )
}

fn drop_table_ignore(conn: &mut OracleThinSession, table: &str) {
    let _ = conn.query_drop(&format!("DROP TABLE {table} PURGE"));
}

fn select_count(conn: &mut OracleThinSession, table: &str) -> i64 {
    let result = conn
        .query_described_fetch_all(format!("SELECT COUNT(*) FROM {table}"), 1)
        .expect("select count");
    let value = result
        .result
        .rows
        .first()
        .and_then(|row| row.first())
        .expect("count row");
    match value {
        OracleValue::Number(value) => value.parse::<i64>().expect("numeric count"),
        other => panic!("expected NUMBER count, got {other:?}"),
    }
}

fn rows_to_strings(rows: &[Vec<OracleValue>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(value_to_string).collect())
        .collect()
}

fn value_to_string(value: &OracleValue) -> String {
    match value {
        OracleValue::Number(value) | OracleValue::Text(value) => value.clone(),
        other => panic!("unexpected test value {other:?}"),
    }
}

fn expected_rows(start: i32, end: i32) -> Vec<Vec<String>> {
    (start..=end)
        .map(|value| vec![value.to_string(), format!("R{value}")])
        .collect()
}

struct TableDropGuard {
    config: OracleThinConfig,
    table: String,
}

impl TableDropGuard {
    fn new(config: OracleThinConfig, table: String) -> Self {
        Self { config, table }
    }
}

impl Drop for TableDropGuard {
    fn drop(&mut self) {
        if let Ok(mut conn) = OracleThinSession::connect(self.config.clone()) {
            drop_table_ignore(&mut conn, &self.table);
        }
    }
}
