#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the object browser's `Export Data...` on Oracle.
//
// Exporting a table used to write an extra leading `ROWID` column into the
// file: the OCI export ran the SELECT through the statement pipeline, which
// injects `ROWIDTOCHAR(t.ROWID) AS SQ_INTERNAL_ROWID` so a grid can be edited.
// An export has no grid, so that column is pure noise in the output.
//
// This binary reads a table through the very functions
// `ObjectBrowser::load_table_rows` calls (OCI and Thin) and asserts the
// returned columns are exactly the table's own.
//
// Usage:   cargo run --bin verify_object_export_live
// Env/DB:  docs/oracle.md (127.0.0.1:1521 service FREE, system/password).
//          OCI needs ORACLE_CLIENT_LIB_DIR pointing at a valid Instant Client.

use oracle::Connection;
use space_query::db::query::{ObjectBrowser, QueryResult};
use tns_thin::exec::StatementRequest;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 1521;
const SERVICE: &str = "FREE";
const USER: &str = "system";
const PASS: &str = "password";

const TABLE: &str = "SQ_EXPORT_ROWID_T";
const CREATE_SQL: &str =
    "CREATE TABLE SQ_EXPORT_ROWID_T (id NUMBER PRIMARY KEY, name VARCHAR2(30))";
const INSERT_SQL: &str = "INSERT INTO SQ_EXPORT_ROWID_T VALUES (1, 'first')";
const EXPORT_SQL: &str = "SELECT * FROM SQ_EXPORT_ROWID_T";

const THIN_PROTOCOLS: &[u16] = &[314, 315, 318, 319];

const EXPECTED: &[&str] = &["ID", "NAME"];

fn check(label: &str, result: &QueryResult) -> Result<(), String> {
    let columns: Vec<String> = result
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    println!("{label}: columns {columns:?}");
    for row in &result.rows {
        println!("{label}: row {row:?}");
    }
    if columns != EXPECTED {
        return Err(format!("{label}: expected {EXPECTED:?}, got {columns:?}"));
    }
    if result.rows.iter().any(|row| row.len() != EXPECTED.len()) {
        return Err(format!("{label}: a row carries more cells than columns"));
    }
    println!("{label}: OK (no ROWID column)");
    Ok(())
}

fn verify_oci() -> Result<(), String> {
    let conn = Connection::connect(USER, PASS, format!("//{HOST}:{PORT}/{SERVICE}"))
        .map_err(|e| format!("OCI connect: {e}"))?;
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    conn.execute(CREATE_SQL, &[])
        .map_err(|e| format!("OCI create: {e}"))?;
    conn.execute(INSERT_SQL, &[])
        .map_err(|e| format!("OCI insert: {e}"))?;
    conn.commit().map_err(|e| format!("OCI commit: {e}"))?;
    let result = ObjectBrowser::execute_oci_query(&conn, EXPORT_SQL)
        .map_err(|e| format!("OCI execute_oci_query: {e}"));
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    check("OCI", &result?)
}

fn thin_config(protocol: u16) -> OracleThinConfig {
    let mut config = OracleThinConfig::new(
        ConnectTarget::service_name(HOST.to_string(), PORT, SERVICE.to_string()),
        USER.to_string(),
        PASS.to_string(),
    );
    config.connect_options.desired_protocol_version = protocol;
    config.connect_options.minimum_protocol_version = protocol;
    config.connect_options.disable_oob_probe = true;
    config
}

fn verify_thin(protocol: u16) -> Result<(), String> {
    let mut session = OracleThinSession::connect(thin_config(protocol))
        .map_err(|e| format!("thin {protocol} connect: {e}"))?;
    println!(
        "thin requested protocol {protocol}, negotiated {:?}",
        session.capabilities().protocol_version
    );
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP TABLE {TABLE} PURGE")),
        0,
    );
    session
        .execute(&StatementRequest::statement(CREATE_SQL.to_string()), 0)
        .map_err(|e| format!("thin {protocol} create: {e}"))?;
    session
        .execute(&StatementRequest::statement(INSERT_SQL.to_string()), 0)
        .map_err(|e| format!("thin {protocol} insert: {e}"))?;
    session
        .execute(&StatementRequest::statement("COMMIT".to_string()), 0)
        .map_err(|e| format!("thin {protocol} commit: {e}"))?;
    let result = ObjectBrowser::execute_thin_query(&mut session, EXPORT_SQL)
        .map_err(|e| format!("thin {protocol} execute_thin_query: {e}"));
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP TABLE {TABLE} PURGE")),
        0,
    );
    check(&format!("Thin proto {protocol}"), &result?)
}

fn main() {
    let mut failures = Vec::new();

    println!("\n########## OCI ##########");
    if let Err(e) = verify_oci() {
        eprintln!("FAIL [OCI]: {e}");
        failures.push("OCI".to_string());
    }

    for &protocol in THIN_PROTOCOLS {
        println!("\n########## Thin protocol {protocol} ##########");
        if let Err(e) = verify_thin(protocol) {
            eprintln!("FAIL [Thin {protocol}]: {e}");
            failures.push(format!("Thin {protocol}"));
        }
    }

    println!("\n==================== SUMMARY ====================");
    if failures.is_empty() {
        println!("ALL TARGETS PASSED (OCI + Thin 314/315/318/319)");
    } else {
        println!("FAILED: {}", failures.join(", "));
        std::process::exit(1);
    }
}
