#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the DBMS_METADATA session-transform fix.
//
// Generating CREATE DDL via `DBMS_METADATA.GET_DDL` used to emit physical
// storage clauses the user never authored (segment attributes, STORAGE,
// TABLESPACE). The fix sets SESSION_TRANSFORM params (SEGMENT_ATTRIBUTES /
// STORAGE / TABLESPACE = FALSE) before fetching DDL, on both the OCI and Thin
// paths.
//
// This binary creates a plain table, regenerates its DDL through the real
// `ObjectBrowser` helpers, and asserts none of those clauses leak back in. It
// checks OCI once (no protocol concept) and Thin across protocols 314/315/318/319.
//
// Usage:   cargo run --bin verify_ddl_transform_live
// Env/DB:  docs/oracle.md (127.0.0.1:1521 service FREE, system/password).
//          OCI needs ORACLE_CLIENT_LIB_DIR pointing at a valid Instant Client.

use oracle::Connection;
use space_query::db::query::ObjectBrowser;
use tns_thin::exec::StatementRequest;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 1521;
const SERVICE: &str = "FREE";
const USER: &str = "system";
const PASS: &str = "password";

const TABLE: &str = "SQ_DDL_XFORM_T";
const CREATE_SQL: &str = "CREATE TABLE SQ_DDL_XFORM_T (\
    id NUMBER PRIMARY KEY, \
    name VARCHAR2(30) NOT NULL, \
    created DATE DEFAULT SYSDATE)";

const THIN_PROTOCOLS: &[u16] = &[314, 315, 318, 319];

/// Clauses DBMS_METADATA injects by default that the user never authored.
const FORBIDDEN: &[&str] = &[
    "SEGMENT CREATION",
    "PCTFREE",
    "PCTUSED",
    "INITRANS",
    "MAXTRANS",
    "STORAGE",
    "TABLESPACE",
];

fn check_ddl(label: &str, ddl: &str) -> Result<(), String> {
    println!(
        "\n----- {label} DDL -----\n{}\n-----------------------",
        ddl.trim()
    );
    let upper = ddl.to_uppercase();
    if !upper.contains("CREATE TABLE") {
        return Err(format!("{label}: output is not a CREATE TABLE"));
    }
    let hits: Vec<&str> = FORBIDDEN
        .iter()
        .copied()
        .filter(|clause| upper.contains(clause))
        .collect();
    if hits.is_empty() {
        println!("{label}: OK (no segment/storage/tablespace clauses)");
        Ok(())
    } else {
        Err(format!("{label}: forbidden clauses present: {hits:?}"))
    }
}

fn verify_oci() -> Result<(), String> {
    let conn = Connection::connect(USER, PASS, format!("//{HOST}:{PORT}/{SERVICE}"))
        .map_err(|e| format!("OCI connect: {e}"))?;
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    conn.execute(CREATE_SQL, &[])
        .map_err(|e| format!("OCI create: {e}"))?;
    let ddl =
        ObjectBrowser::get_table_ddl(&conn, TABLE).map_err(|e| format!("OCI get_table_ddl: {e}"));
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    check_ddl("OCI", &ddl?)
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
    let negotiated = session.capabilities().protocol_version;
    println!("thin requested protocol {protocol}, negotiated {negotiated:?}");
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP TABLE {TABLE} PURGE")),
        0,
    );
    session
        .execute(&StatementRequest::statement(CREATE_SQL.to_string()), 0)
        .map_err(|e| format!("thin {protocol} create: {e}"))?;
    let ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "TABLE", TABLE)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl: {e}"));
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP TABLE {TABLE} PURGE")),
        0,
    );
    check_ddl(&format!("Thin proto {protocol}"), &ddl?)
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
