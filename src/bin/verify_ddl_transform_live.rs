#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the DBMS_METADATA transform the app applies to DDL.
//
// Generating CREATE DDL via `DBMS_METADATA` used to emit physical storage
// clauses the user never authored (segment attributes, STORAGE, TABLESPACE).
// The app turns those off, on both the OCI and Thin paths.
//
// It must do so WITHOUT touching the session. Setting the params on
// `DBMS_METADATA.SESSION_TRANSFORM` (what `GET_DDL` obeys) writes them onto the
// session, and an Oracle pool hands a session back exactly as its last user
// left it — so a query tab that later picked up that pooled session got the
// app's preference applied to its own `GET_DDL`. The params now ride on a
// metadata HANDLE instead, so both halves are checked here: the generated DDL
// has no storage clauses, and a plain `GET_DDL` on the SAME session right
// afterwards still has them.
//
// This binary creates a plain table, regenerates its DDL through the real
// `ObjectBrowser` helpers, and asserts both. It checks OCI once (no protocol
// concept) and Thin across protocols 314/315/318/319.
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

/// A table is not enough: `DBMS_METADATA` walks a different path per object
/// type, and the app generates DDL for every type the browser tree shows.
const VIEW: &str = "SQ_DDL_XFORM_V";
const CREATE_VIEW_SQL: &str =
    "CREATE OR REPLACE VIEW SQ_DDL_XFORM_V AS SELECT id, name FROM SQ_DDL_XFORM_T";
const PROC: &str = "SQ_DDL_XFORM_P";
const CREATE_PROC_SQL: &str =
    "CREATE OR REPLACE PROCEDURE SQ_DDL_XFORM_P (p_id IN NUMBER) AS BEGIN NULL; END;";

const THIN_PROTOCOLS: &[u16] = &[314, 315, 318, 319];

/// A plain `GET_DDL` on the session the app just generated DDL on. Its output
/// must STILL carry the storage clauses: if it does not, the app changed this
/// session's `SESSION_TRANSFORM` and the next tab to be handed this pooled
/// session would inherit that.
const SESSION_LEAK_PROBE_SQL: &str = "SELECT TO_CHAR(SUBSTR(DBMS_METADATA.GET_DDL(\'TABLE\', \
\'SQ_DDL_XFORM_T\', SYS_CONTEXT(\'USERENV\', \'CURRENT_SCHEMA\')), 1, 3000)) FROM DUAL";

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

/// The session must be exactly as the app found it.
fn check_session_untouched(label: &str, probe_ddl: &str) -> Result<(), String> {
    let upper = probe_ddl.to_uppercase();
    let carried: Vec<&str> = FORBIDDEN
        .iter()
        .copied()
        .filter(|clause| upper.contains(clause))
        .collect();
    if carried.is_empty() {
        return Err(format!(
            "{label}: generating DDL changed this session's DBMS_METADATA transform — a plain \
             GET_DDL on it no longer emits any of {FORBIDDEN:?}, so the next tab handed this \
             pooled session would inherit the app's preference:\n{}",
            probe_ddl.trim()
        ));
    }
    println!("{label}: OK (session transform untouched; plain GET_DDL still emits {carried:?})");
    Ok(())
}

/// Object types other than TABLE only have to come back at all: the transform
/// params they carry are the same ones, and a hang or an error here is the
/// signal that matters.
fn check_object_ddl(label: &str, ddl: &str, expected: &str) -> Result<(), String> {
    let upper = ddl.to_uppercase();
    if !upper.contains(expected) {
        return Err(format!(
            "{label}: expected DDL containing {expected:?}, got:\n{}",
            ddl.trim()
        ));
    }
    println!("{label}: OK ({} bytes)", ddl.len());
    Ok(())
}

fn verify_oci() -> Result<(), String> {
    let conn = Connection::connect(USER, PASS, format!("//{HOST}:{PORT}/{SERVICE}"))
        .map_err(|e| format!("OCI connect: {e}"))?;
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    conn.execute(CREATE_SQL, &[])
        .map_err(|e| format!("OCI create: {e}"))?;
    conn.execute(CREATE_VIEW_SQL, &[])
        .map_err(|e| format!("OCI create view: {e}"))?;
    conn.execute(CREATE_PROC_SQL, &[])
        .map_err(|e| format!("OCI create procedure: {e}"))?;
    let view_ddl =
        ObjectBrowser::get_view_ddl(&conn, VIEW).map_err(|e| format!("OCI get_view_ddl: {e}"));
    let proc_ddl = ObjectBrowser::get_procedure_ddl(&conn, PROC)
        .map_err(|e| format!("OCI get_procedure_ddl: {e}"));
    let ddl =
        ObjectBrowser::get_table_ddl(&conn, TABLE).map_err(|e| format!("OCI get_table_ddl: {e}"));
    // On the SAME session, before the table goes away.
    let probe = conn
        .query_row_as::<String>(SESSION_LEAK_PROBE_SQL, &[])
        .map_err(|e| format!("OCI session leak probe: {e}"));
    let _ = conn.execute(&format!("DROP PROCEDURE {PROC}"), &[]);
    let _ = conn.execute(&format!("DROP VIEW {VIEW}"), &[]);
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    check_ddl("OCI", &ddl?)?;
    check_object_ddl("OCI view", &view_ddl?, "CREATE OR REPLACE")?;
    check_object_ddl("OCI procedure", &proc_ddl?, "PROCEDURE")?;
    check_session_untouched("OCI", &probe?)
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
    let _ = session.execute(&StatementRequest::statement(CREATE_VIEW_SQL.to_string()), 0);
    let _ = session.execute(&StatementRequest::statement(CREATE_PROC_SQL.to_string()), 0);
    let ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "TABLE", TABLE)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl: {e}"));
    let view_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "VIEW", VIEW)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl(VIEW): {e}"));
    let proc_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "PROCEDURE", PROC)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl(PROCEDURE): {e}"));
    // On the SAME session, before the table goes away.
    let probe = space_query::db::DatabaseConnection::oracle_thin_select_one_text_for_test(
        &mut session,
        SESSION_LEAK_PROBE_SQL,
    )
    .map_err(|e| format!("thin {protocol} session leak probe: {e}"))
    .map(|value| value.unwrap_or_default());
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP TABLE {TABLE} PURGE")),
        0,
    );
    let _ = session.execute(
        &StatementRequest::statement(format!("DROP PROCEDURE {PROC}")),
        0,
    );
    let _ = session.execute(&StatementRequest::statement(format!("DROP VIEW {VIEW}")), 0);
    let label = format!("Thin proto {protocol}");
    check_ddl(&label, &ddl?)?;
    check_object_ddl(&format!("{label} view"), &view_ddl?, "CREATE OR REPLACE")?;
    check_object_ddl(&format!("{label} procedure"), &proc_ddl?, "PROCEDURE")?;
    check_session_untouched(&label, &probe?)
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
