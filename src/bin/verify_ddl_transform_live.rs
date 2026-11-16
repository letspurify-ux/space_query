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
// concept) and Thin across every protocol of 314/315/318/319 the server
// speaks: an 11.2 listener accepts 314 and nothing newer.
//
// The DDL comes back through an OUT CLOB, so a third check reads one that is
// long: past 32,767 bytes (the longest VARCHAR2 bind, which a CLOB bound as
// one cannot exceed) and with characters outside the BMP lying across the
// places a LOB read may be cut.
//
// Usage:   cargo run --bin verify_ddl_transform_live
// Env/DB:  docs/oracle.md (127.0.0.1:1521 service FREE, system/password), or
//          ORACLE_TEST_HOST / _PORT / _SERVICE_NAME / _USERNAME / _PASSWORD.
//          OCI needs ORACLE_CLIENT_LIB_DIR pointing at a valid Instant Client.

use oracle::Connection;
use space_query::db::query::ObjectBrowser;
use std::env;
use tns_thin::exec::StatementRequest;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

/// The server under test: host, port, service, user, password.
struct Target {
    host: String,
    port: u16,
    service: String,
    user: String,
    pass: String,
}

impl Target {
    fn from_env() -> Self {
        Self {
            host: env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: env::var("ORACLE_TEST_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1521),
            service: env::var("ORACLE_TEST_SERVICE_NAME")
                .or_else(|_| env::var("ORACLE_TEST_SERVICE"))
                .unwrap_or_else(|_| "FREE".into()),
            user: env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".into()),
            pass: env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".into()),
        }
    }
}

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

/// A procedure whose DDL is LONG, in the two ways an OUT CLOB can lose text.
const LONG_PROC: &str = "SQ_DDL_XFORM_LONG_P";

/// One character outside the BMP: two UTF-16 code units, which is how a CLOB
/// counts, so a read cut at a code-unit offset can land inside it.
const ASTRAL: char = '\u{1F600}';
const ASTRAL_PER_LINE: usize = 400;
const ASTRAL_LINES_PER_RUN: usize = 13;

/// How many code units one LOB read may be cut at. It is the OCI driver's own
/// chunk; a boundary of any other reader is a multiple or is never reached.
const LOB_READ_UNITS: usize = 8_192;

/// The comment body the long procedure carries: two runs of astral characters
/// with ONE ordinary character between them.
///
/// Every line of a run is an EVEN number of code units (the characters, a
/// newline and a space), so a run keeps one parity from its first character to
/// its last, and each run is over 10,000 units: the first lies across unit
/// 8,192 and the second across 16,384, the first two places a read ends. The
/// single `x` between them shifts the second run by one unit, so whichever
/// parity the text before the comment has, one of the two runs has a
/// character sitting ACROSS its boundary. Lines, not one long line: a source
/// line is stored in 4,000 bytes, and 11.2 does not survive generating DDL
/// from one that is ten times that.
fn astral_comment_body() -> String {
    let line: String = std::iter::repeat_n(ASTRAL, ASTRAL_PER_LINE)
        .chain("\n ".chars())
        .collect();
    let run = line.repeat(ASTRAL_LINES_PER_RUN);
    format!("{run}x{run}")
}

/// Long past 32,767 bytes as well: the comment alone is over 41,000.
fn create_long_proc_sql() -> String {
    format!(
        "CREATE OR REPLACE PROCEDURE {LONG_PROC} AS\n/* {}*/\nBEGIN NULL; END;",
        astral_comment_body()
    )
}

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

/// The long procedure's DDL has to be the text that was compiled: all of it,
/// and every astral character still one character.
fn check_long_ddl(label: &str, ddl: &str) -> Result<(), String> {
    if ddl.len() <= 32_767 {
        return Err(format!(
            "{label}: the long DDL came back as {} bytes; it is longer than 32,767",
            ddl.len()
        ));
    }
    let expected_astral = ASTRAL_PER_LINE * ASTRAL_LINES_PER_RUN * 2;
    if !ddl.contains(&astral_comment_body()) {
        let astral = ddl.chars().filter(|ch| *ch == ASTRAL).count();
        return Err(format!(
            "{label}: the long DDL lost text — {astral} of {expected_astral} astral characters \
             survived, {} bytes in all",
            ddl.len()
        ));
    }
    // The fixture has to have put a character across a read boundary on THIS
    // server, or the check above proved nothing about cut reads.
    let mut units = 0usize;
    let straddled = ddl.chars().any(|ch| {
        let first_unit = units;
        units += ch.len_utf16();
        ch.len_utf16() == 2 && (first_unit + 1).is_multiple_of(LOB_READ_UNITS)
    });
    if !straddled {
        return Err(format!(
            "{label}: no astral character lies across a {LOB_READ_UNITS}-unit read boundary in \
             this server's DDL, so the fixture no longer tests a cut read"
        ));
    }
    println!(
        "{label}: OK (long DDL whole: {} bytes, {expected_astral} astral characters, one across \
         a read boundary)",
        ddl.len()
    );
    Ok(())
}

fn verify_oci(target: &Target) -> Result<(), String> {
    let conn = Connection::connect(
        &target.user,
        &target.pass,
        format!("//{}:{}/{}", target.host, target.port, target.service),
    )
    .map_err(|e| format!("OCI connect: {e}"))?;
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    conn.execute(CREATE_SQL, &[])
        .map_err(|e| format!("OCI create: {e}"))?;
    conn.execute(CREATE_VIEW_SQL, &[])
        .map_err(|e| format!("OCI create view: {e}"))?;
    conn.execute(CREATE_PROC_SQL, &[])
        .map_err(|e| format!("OCI create procedure: {e}"))?;
    conn.execute(&create_long_proc_sql(), &[])
        .map_err(|e| format!("OCI create long procedure: {e}"))?;
    let view_ddl =
        ObjectBrowser::get_view_ddl(&conn, VIEW).map_err(|e| format!("OCI get_view_ddl: {e}"));
    let proc_ddl = ObjectBrowser::get_procedure_ddl(&conn, PROC)
        .map_err(|e| format!("OCI get_procedure_ddl: {e}"));
    let long_ddl = ObjectBrowser::get_procedure_ddl(&conn, LONG_PROC)
        .map_err(|e| format!("OCI get_procedure_ddl(long): {e}"));
    let ddl =
        ObjectBrowser::get_table_ddl(&conn, TABLE).map_err(|e| format!("OCI get_table_ddl: {e}"));
    // On the SAME session, before the table goes away.
    let probe = conn
        .query_row_as::<String>(SESSION_LEAK_PROBE_SQL, &[])
        .map_err(|e| format!("OCI session leak probe: {e}"));
    let _ = conn.execute(&format!("DROP PROCEDURE {LONG_PROC}"), &[]);
    let _ = conn.execute(&format!("DROP PROCEDURE {PROC}"), &[]);
    let _ = conn.execute(&format!("DROP VIEW {VIEW}"), &[]);
    let _ = conn.execute(&format!("DROP TABLE {TABLE} PURGE"), &[]);
    check_ddl("OCI", &ddl?)?;
    check_object_ddl("OCI view", &view_ddl?, "CREATE OR REPLACE")?;
    check_object_ddl("OCI procedure", &proc_ddl?, "PROCEDURE")?;
    check_long_ddl("OCI", &long_ddl?)?;
    check_session_untouched("OCI", &probe?)
}

/// A thin session on `protocol` exactly, or — with `None` — on whatever the
/// listener and the driver agree on.
fn thin_config(target: &Target, protocol: Option<u16>) -> OracleThinConfig {
    let mut config = OracleThinConfig::new(
        ConnectTarget::service_name(target.host.clone(), target.port, target.service.clone()),
        target.user.clone(),
        target.pass.clone(),
    );
    if let Some(protocol) = protocol {
        config.connect_options.desired_protocol_version = protocol;
        config.connect_options.minimum_protocol_version = protocol;
    }
    config.connect_options.disable_oob_probe = true;
    config
}

/// The newest protocol this server speaks. A release older than a protocol
/// cannot be asked for it — an 11.2 listener accepts 314 and refuses the rest —
/// and that is the server's limit, not a failure of what is checked here.
fn newest_protocol(target: &Target) -> Result<u16, String> {
    let session = OracleThinSession::connect(thin_config(target, None))
        .map_err(|e| format!("thin connect: {e}"))?;
    session
        .capabilities()
        .protocol_version
        .ok_or_else(|| "thin session reported no protocol version".to_string())
}

fn verify_thin(target: &Target, protocol: u16) -> Result<(), String> {
    let mut session = OracleThinSession::connect(thin_config(target, Some(protocol)))
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
    session
        .execute(&StatementRequest::statement(create_long_proc_sql()), 0)
        .map_err(|e| format!("thin {protocol} create long procedure: {e}"))?;
    let ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "TABLE", TABLE)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl: {e}"));
    let view_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "VIEW", VIEW)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl(VIEW): {e}"));
    let proc_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "PROCEDURE", PROC)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl(PROCEDURE): {e}"));
    let long_ddl = ObjectBrowser::get_thin_object_ddl(&mut session, "PROCEDURE", LONG_PROC)
        .map_err(|e| format!("thin {protocol} get_thin_object_ddl(long PROCEDURE): {e}"));
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
        &StatementRequest::statement(format!("DROP PROCEDURE {LONG_PROC}")),
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
    check_long_ddl(&label, &long_ddl?)?;
    check_session_untouched(&label, &probe?)
}

fn main() {
    let target = Target::from_env();
    let mut failures = Vec::new();
    let mut passed = Vec::new();

    println!("\n########## OCI ##########");
    match verify_oci(&target) {
        Ok(()) => passed.push("OCI".to_string()),
        Err(e) => {
            eprintln!("FAIL [OCI]: {e}");
            failures.push("OCI".to_string());
        }
    }

    match newest_protocol(&target) {
        Ok(newest) => {
            for &protocol in THIN_PROTOCOLS {
                if protocol > newest {
                    println!(
                        "\n########## Thin protocol {protocol}: not spoken by this server \
                         (newest {newest}) ##########"
                    );
                    continue;
                }
                println!("\n########## Thin protocol {protocol} ##########");
                match verify_thin(&target, protocol) {
                    Ok(()) => passed.push(format!("Thin {protocol}")),
                    Err(e) => {
                        eprintln!("FAIL [Thin {protocol}]: {e}");
                        failures.push(format!("Thin {protocol}"));
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("FAIL [Thin]: {e}");
            failures.push("Thin".to_string());
        }
    }

    println!("\n==================== SUMMARY ====================");
    if failures.is_empty() {
        println!("ALL TARGETS PASSED ({})", passed.join(", "));
    } else {
        println!("FAILED: {}", failures.join(", "));
        std::process::exit(1);
    }
}
