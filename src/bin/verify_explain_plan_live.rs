#![allow(clippy::cargo, clippy::pedantic)]

// Live verification for the three features added alongside it — explain plan
// visualization (item 12) and go to declaration / object search (item 10) —
// across every supported backend: Oracle Thin, Oracle OCI, MySQL and MariaDB.
//
// The unit tests in `src/ui/explain_plan.rs` and `src/ui/object_search.rs`
// prove the rendering and the ranking against rows this process made up. What
// only a server can settle is:
//
//   (1) that `EXPLAIN PLAN FOR` + `PLAN_TABLE` actually comes back with the
//       parent links, estimates and predicates the renderer expects, through
//       both Oracle drivers, and that MySQL/MariaDB `EXPLAIN` still hands back
//       its own columns;
//   (2) that the drawn tree matches the parent chain the server reported —
//       one root, every parent resolvable, connector depth equal to chain
//       depth;
//   (3) that a name typed in the editor resolves to a real object and its
//       source really opens, for every object type each backend supports.
//
// Everything runs through the production path: `SqlEditorWidget::explain_current`
// (what F6 calls) and `ObjectBrowserWidget::open_declaration_for_sql_selection`
// (what Ctrl+B calls), with the same callbacks the main window installs.
//
// Usage: verify_explain_plan_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time.

use fltk::{app, input::IntInput};
use space_query::db::{
    ConnectionInfo, DatabaseConnection, DatabaseType, DbConnection, OracleDriverMode,
};
use space_query::ui::explain_plan::{plan_grid, ExplainPlanData, PlanNode};
use space_query::ui::intellisense::IntellisenseData;
use space_query::ui::object_browser::{ObjectBrowserWidget, ObjectCache, SqlAction};
use space_query::ui::object_search::{search, MAX_OBJECT_SEARCH_HITS};
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PARENT_TABLE: &str = "OQT_PLAN_DEPT";
const CHILD_TABLE: &str = "OQT_PLAN_EMP";
const VIEW_NAME: &str = "OQT_PLAN_V";
const PROC_NAME: &str = "OQT_PLAN_P";
const FUNC_NAME: &str = "OQT_PLAN_F";
const PACKAGE_NAME: &str = "OQT_PLAN_PKG";

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

    /// Objects covering every declaration kind the backend can open.
    fn setup_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!(
                    "CREATE TABLE {PARENT_TABLE} (DEPTNO NUMBER PRIMARY KEY, DNAME VARCHAR2(30))"
                ),
                format!(
                    "CREATE TABLE {CHILD_TABLE} (EMPNO NUMBER PRIMARY KEY, \
                     DEPTNO NUMBER, SAL NUMBER(9,2))"
                ),
                format!(
                    "CREATE OR REPLACE VIEW {VIEW_NAME} AS \
                     SELECT e.EMPNO, d.DNAME FROM {CHILD_TABLE} e \
                     JOIN {PARENT_TABLE} d ON d.DEPTNO = e.DEPTNO"
                ),
                format!(
                    "CREATE OR REPLACE PROCEDURE {PROC_NAME} (p_id IN NUMBER) AS \
                     BEGIN NULL; END;"
                ),
                format!(
                    "CREATE OR REPLACE FUNCTION {FUNC_NAME} RETURN NUMBER AS \
                     BEGIN RETURN 1; END;"
                ),
                format!(
                    "CREATE OR REPLACE PACKAGE {PACKAGE_NAME} AS \
                     PROCEDURE TOUCH_ROW(p_id IN NUMBER); END {PACKAGE_NAME};"
                ),
                format!(
                    "CREATE OR REPLACE PACKAGE BODY {PACKAGE_NAME} AS \
                     PROCEDURE TOUCH_ROW(p_id IN NUMBER) IS BEGIN NULL; END; \
                     END {PACKAGE_NAME};"
                ),
            ]
        } else {
            vec![
                format!("CREATE TABLE {PARENT_TABLE} (DEPTNO INT PRIMARY KEY, DNAME VARCHAR(30))"),
                format!(
                    "CREATE TABLE {CHILD_TABLE} (EMPNO INT PRIMARY KEY, \
                     DEPTNO INT, SAL DECIMAL(9,2))"
                ),
                format!(
                    "CREATE VIEW {VIEW_NAME} AS \
                     SELECT e.EMPNO, d.DNAME FROM {CHILD_TABLE} e \
                     JOIN {PARENT_TABLE} d ON d.DEPTNO = e.DEPTNO"
                ),
                format!("CREATE PROCEDURE {PROC_NAME} (IN p_id INT) BEGIN SELECT p_id; END"),
                format!("CREATE FUNCTION {FUNC_NAME} () RETURNS INT DETERMINISTIC RETURN 1"),
            ]
        }
    }

    /// Keep a blocked DROP from hanging the probe.
    ///
    /// The object browser loads several metadata categories in parallel, and a
    /// job still in flight can hold a lock on the very objects the teardown
    /// drops. Bounding the wait turns that into an ignorable error; the next
    /// run's opening teardown clears whatever survived.
    fn lock_timeout_sql(self) -> &'static str {
        if self.is_oracle() {
            "ALTER SESSION SET ddl_lock_timeout = 5"
        } else {
            "SET SESSION lock_wait_timeout = 5"
        }
    }

    fn teardown_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!("DROP PACKAGE {PACKAGE_NAME}"),
                format!("DROP FUNCTION {FUNC_NAME}"),
                format!("DROP PROCEDURE {PROC_NAME}"),
                format!("DROP VIEW {VIEW_NAME}"),
                format!("DROP TABLE {CHILD_TABLE}"),
                format!("DROP TABLE {PARENT_TABLE}"),
            ]
        } else {
            vec![
                format!("DROP FUNCTION IF EXISTS {FUNC_NAME}"),
                format!("DROP PROCEDURE IF EXISTS {PROC_NAME}"),
                format!("DROP VIEW IF EXISTS {VIEW_NAME}"),
                format!("DROP TABLE IF EXISTS {CHILD_TABLE}"),
                format!("DROP TABLE IF EXISTS {PARENT_TABLE}"),
            ]
        }
    }

    /// A statement with a join and a subquery, so the plan has real structure.
    fn explain_target_sql(self) -> String {
        format!(
            "SELECT d.DNAME, e.EMPNO FROM {PARENT_TABLE} d \
             JOIN {CHILD_TABLE} e ON e.DEPTNO = d.DEPTNO \
             WHERE e.SAL > (SELECT AVG(SAL) FROM {CHILD_TABLE})"
        )
    }

    /// Names Go to Declaration must resolve, with the text their source carries.
    fn declaration_cases(self) -> Vec<(&'static str, &'static str)> {
        let mut cases = vec![
            (PARENT_TABLE, PARENT_TABLE),
            (VIEW_NAME, VIEW_NAME),
            (PROC_NAME, PROC_NAME),
            (FUNC_NAME, FUNC_NAME),
        ];
        if self.is_oracle() {
            cases.push((PACKAGE_NAME, PACKAGE_NAME));
        }
        cases
    }
}

fn progress_inner(event: &QueryProgress) -> &QueryProgress {
    match event {
        QueryProgress::Operation { progress, .. }
        | QueryProgress::StatementOrigin { progress, .. } => progress_inner(progress),
        other => other,
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

/// The explain path reports failure as a message, not a statement result.
fn explain_failure(events: &[QueryProgress]) -> Option<String> {
    events.iter().find_map(|event| match progress_inner(event) {
        QueryProgress::Message { lines, .. }
            if lines
                .iter()
                .any(|line| line.starts_with("Explain plan failed")) =>
        {
            Some(lines.join(" | "))
        }
        _ => None,
    })
}

fn pump_until<F: Fn() -> bool>(label: &str, seconds: u64, pred: F) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !pred() && Instant::now() < deadline {
        if !app::wait() {
            app::check();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    if !pred() {
        return Err(format!("timed out waiting for {label}"));
    }
    let drain = Instant::now() + Duration::from_millis(200);
    while Instant::now() < drain {
        if !app::wait() {
            app::check();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}

struct Harness {
    editor: SqlEditorWidget,
    events: Arc<Mutex<Vec<QueryProgress>>>,
    done: Arc<AtomicBool>,
}

impl Harness {
    fn run(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_sql_text(sql);
        let done = Arc::clone(&self.done);
        pump_until("statement to finish", 120, || done.load(Ordering::SeqCst))?;
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

    /// F6's own path: put the statement in the buffer, then explain it.
    fn explain(&mut self, sql: &str) -> Result<space_query::db::QueryResult, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.editor.set_text(sql);
        self.editor.explain_current();
        let events = Arc::clone(&self.events);
        pump_until("explain plan", 120, || {
            let events = events.lock().unwrap_or_else(|p| p.into_inner());
            events.iter().any(|event| {
                matches!(
                    progress_inner(event),
                    QueryProgress::ExplainPlanOutput { .. }
                )
            }) || explain_failure(&events).is_some()
        })?;
        let events = events.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(error) = explain_failure(&events).or_else(|| first_error(&events)) {
            return Err(error);
        }
        events
            .iter()
            .find_map(|event| match progress_inner(event) {
                QueryProgress::ExplainPlanOutput { result } => Some(result.clone()),
                _ => None,
            })
            .ok_or_else(|| "no explain plan output".to_string())
    }
}

/// Every structural claim the connector drawing rests on.
fn check_tree_shape(nodes: &[PlanNode]) -> Result<(), String> {
    if nodes.is_empty() {
        return Err("plan has no rows".to_string());
    }
    let roots: Vec<i64> = nodes
        .iter()
        .filter(|node| node.parent_id.is_none())
        .map(|node| node.id)
        .collect();
    if roots.len() != 1 {
        return Err(format!("expected exactly one root, found {roots:?}"));
    }

    for node in nodes {
        let Some(parent_id) = node.parent_id else {
            continue;
        };
        if !nodes.iter().any(|other| other.id == parent_id) {
            return Err(format!(
                "step {} points at parent {parent_id}, which is not in the plan",
                node.id
            ));
        }
    }

    // Walking up from every node must reach the root without revisiting a step.
    for node in nodes {
        let mut seen = vec![node.id];
        let mut current = node.parent_id;
        while let Some(parent_id) = current {
            if seen.contains(&parent_id) {
                return Err(format!("parent chain of step {} is a cycle", node.id));
            }
            seen.push(parent_id);
            current = nodes
                .iter()
                .find(|other| other.id == parent_id)
                .and_then(|other| other.parent_id);
        }
    }
    Ok(())
}

/// The drawn indentation must equal the real depth in the parent chain.
fn check_connector_depth(nodes: &[PlanNode], rows: &[Vec<String>]) -> Result<(), String> {
    for (node, row) in nodes.iter().zip(rows) {
        let mut depth = 0usize;
        let mut current = node.parent_id;
        while let Some(parent_id) = current {
            depth += 1;
            current = nodes
                .iter()
                .find(|other| other.id == parent_id)
                .and_then(|other| other.parent_id);
        }
        let operation = row.first().cloned().unwrap_or_default();
        let drawn = operation
            .chars()
            .take_while(|ch| matches!(ch, '│' | '├' | '└' | '─' | ' '))
            .count();
        // Every level draws exactly three columns of connector.
        let expected = depth * 3;
        if drawn != expected {
            return Err(format!(
                "step {} sits at depth {depth} but was drawn with {drawn} connector columns: {operation:?}",
                node.id
            ));
        }
    }
    Ok(())
}

/// How many transactions the connection's SHARED LIVE session is carrying,
/// read on that very session.
///
/// `EXPLAIN PLAN FOR` writes into `PLAN_TABLE` on this session, which no query
/// tab owns: the tab's auto-commit governs its own pooled session and the
/// Commit/Rollback buttons act on the tab's retained session by design, so
/// nothing else in the app would ever resolve that write. Reading system views
/// starts no transaction of its own, so this probe cannot create what it looks
/// for.
fn live_session_open_transactions(shared: &Arc<Mutex<DatabaseConnection>>) -> Result<u32, String> {
    const SQL: &str = "SELECT TO_CHAR(COUNT(*)) FROM v$transaction t, v$session s \
                       WHERE s.sid = SYS_CONTEXT('USERENV', 'SID') AND t.ses_addr = s.saddr";
    let db_conn = {
        let guard = shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .get_db_connection()
            .ok_or_else(|| "no live connection to probe".to_string())?
    };
    let text = match db_conn {
        DbConnection::Oracle(conn) => conn
            .query_row_as::<String>(SQL, &[])
            .map_err(|err| format!("live transaction probe (oci): {err}"))?,
        DbConnection::OracleThin(session) => {
            let mut session = session
                .lock()
                .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
            DatabaseConnection::oracle_thin_select_one_text_for_test(&mut session, SQL)
                .map_err(|err| format!("live transaction probe (thin): {err}"))?
                .unwrap_or_default()
        }
        DbConnection::MySQL { .. } => {
            return Err("live transaction probe is Oracle-only".to_string())
        }
    };
    text.trim()
        .parse::<u32>()
        .map_err(|err| format!("live transaction probe returned {text:?}: {err}"))
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

    let _ = h.run(target.lock_timeout_sql());
    for sql in target.teardown_sql() {
        let _ = h.run(&sql);
    }
    for sql in target.setup_sql() {
        h.run(&sql).map_err(|e| format!("setup ({sql}): {e}"))?;
    }
    let _ = h.run("COMMIT");

    // ---- item 12: explain plan -------------------------------------------
    let plan_result = h.explain(&target.explain_target_sql())?;
    if plan_result.rows.is_empty() {
        return Err("explain plan produced no rows".to_string());
    }
    let column_names: Vec<&str> = plan_result
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    println!("plan columns: {column_names:?}");
    for row in &plan_result.rows {
        println!("  {}", row.join(" | "));
    }

    if target.is_oracle() {
        if column_names
            != [
                "Operation",
                "Object",
                "Rows",
                "Bytes",
                "Cost",
                "Cost %",
                "Predicates",
            ]
        {
            return Err(format!("unexpected Oracle plan columns: {column_names:?}"));
        }
        // Re-derive the nodes the same way the UI did, then check the drawing
        // against them.
        let steps = plan_step_labels(&plan_result.rows);
        if steps.is_empty() {
            return Err("Oracle plan rendered no steps".to_string());
        }
        if !plan_result.rows.iter().any(|row| {
            row.first()
                .is_some_and(|op| op.contains("SELECT STATEMENT"))
        }) {
            return Err("Oracle plan is missing its SELECT STATEMENT root".to_string());
        }
        if !plan_result.rows.iter().any(|row| {
            row.first()
                .is_some_and(|op| op.contains("JOIN") || op.contains("NESTED LOOPS"))
        }) {
            return Err("Oracle plan of a join has no join step".to_string());
        }
        if !plan_result
            .rows
            .iter()
            .any(|row| row.get(6).is_some_and(|value| !value.is_empty()))
        {
            return Err("Oracle plan carried no predicates".to_string());
        }
        if !plan_result
            .rows
            .iter()
            .any(|row| row.get(5).is_some_and(|value| value.contains('%')))
        {
            return Err("Oracle plan carried no cost share".to_string());
        }
        // The plan came from an INSERT into PLAN_TABLE on the shared live
        // session. Nothing in the app would ever commit or roll that back —
        // auto-commit belongs to the tab's own pooled session and the
        // Commit/Rollback buttons deliberately act on the tab's retained
        // session — so the statement that wrote has to take it back before it
        // returns, or every F6 adds rows and locks to one transaction that
        // stays open for the life of the connection.
        let open_transactions = live_session_open_transactions(&shared)?;
        if open_transactions != 0 {
            return Err(format!(
                "the explain left {open_transactions} open transaction(s) on the shared live session"
            ));
        }
        println!("  OK  the explain left no open transaction on the shared live session");
    } else {
        for expected in ["id", "select_type", "table"] {
            if !column_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(expected))
            {
                return Err(format!(
                    "MySQL-family plan is missing the server's own `{expected}` column: \
                     {column_names:?}"
                ));
            }
        }
        if !column_names.contains(&"Rows %") {
            return Err("MySQL-family plan gained no row share column".to_string());
        }
    }

    // A statement typed with its terminator is the normal case in the editor,
    // and Oracle rejects `EXPLAIN PLAN FOR ... ;` outright. MySQL normalizes it
    // away; Oracle has to too.
    let terminated = format!("{};", target.explain_target_sql());
    let terminated_plan = h
        .explain(&terminated)
        .map_err(|e| format!("explain of a statement ending in a semicolon: {e}"))?;
    if terminated_plan.rows.is_empty() {
        return Err("a statement ending in a semicolon produced no plan".to_string());
    }
    println!("a trailing semicolon still explains");

    // The same structural checks, on a plan whose parent links are known —
    // this is what proves the drawing, not just that a plan came back.
    let sample = vec![
        PlanNode {
            id: 0,
            parent_id: None,
            operation: "SELECT STATEMENT".to_string(),
            cost: Some(20),
            ..PlanNode::default()
        },
        PlanNode {
            id: 1,
            parent_id: Some(0),
            operation: "HASH JOIN".to_string(),
            cost: Some(20),
            ..PlanNode::default()
        },
        PlanNode {
            id: 2,
            parent_id: Some(1),
            operation: "TABLE ACCESS FULL".to_string(),
            cost: Some(5),
            ..PlanNode::default()
        },
        PlanNode {
            id: 3,
            parent_id: Some(1),
            operation: "TABLE ACCESS FULL".to_string(),
            cost: Some(12),
            ..PlanNode::default()
        },
    ];
    check_tree_shape(&sample)?;
    let (_, sample_rows) = plan_grid(&ExplainPlanData::Tree(sample.clone()));
    check_connector_depth(&sample, &sample_rows)?;
    println!("connector depth matches the parent chain");

    // ---- the tab's Read only pin governs F6 too --------------------------
    // `EXPLAIN PLAN FOR` inserts into PLAN_TABLE, so on Oracle a tab pinned
    // Read only must be refused exactly as Ctrl+Enter would refuse a write —
    // the pin cannot mean one thing for one button and another for the next.
    // On the MySQL family `EXPLAIN` is a read, so the same pin must NOT block
    // it: over-blocking would be its own bug.
    h.editor
        .set_tab_transaction_mode(space_query::db::TransactionMode::new(
            space_query::db::TransactionIsolation::Default,
            space_query::db::TransactionAccessMode::ReadOnly,
        ));
    let pinned = h.explain(&target.explain_target_sql());
    h.editor.clear_tab_transaction_mode_override();
    if target.is_oracle() {
        match pinned {
            Ok(_) => {
                return Err(
                    "a Read only tab explained anyway: EXPLAIN PLAN writes to PLAN_TABLE"
                        .to_string(),
                )
            }
            Err(message) if message.to_lowercase().contains("read-only mode blocks") => {
                println!("  OK  a Read only tab refuses F6 with the same message as a write");
            }
            Err(message) => {
                return Err(format!(
                    "a Read only tab refused F6, but not with the read-only message: {message}"
                ))
            }
        }
        // ... and the identical explain runs once the pin is gone, which is
        // what proves the refusal was the pin's doing.
        h.explain(&target.explain_target_sql())
            .map_err(|e| format!("explain after unpinning: {e}"))?;
        println!("  OK  the same explain runs once the pin is gone");
    } else {
        pinned.map_err(|e| {
            format!("a Read only pin blocked a MySQL-family EXPLAIN, which is a read: {e}")
        })?;
        println!("  OK  a Read only tab still explains (EXPLAIN is a read here)");
    }

    // ---- item 10: object search + go to declaration -----------------------
    // Scoped: the browser holds a pooled metadata session, and the teardown
    // below drops the very objects that session has open. Letting it go out of
    // scope first keeps the DROPs from waiting on a lock the probe itself holds.
    verify_object_declarations(target, Arc::clone(&shared))?;

    let _ = h.run(target.lock_timeout_sql());
    for sql in target.teardown_sql() {
        let _ = h.run(&sql);
    }
    let _ = h.run("COMMIT");
    Ok(())
}

fn verify_object_declarations(
    target: Target,
    shared: Arc<Mutex<DatabaseConnection>>,
) -> Result<(), String> {
    let mut browser = ObjectBrowserWidget::new(0, 0, 320, 480, Arc::clone(&shared));
    let opened: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let opened = Arc::clone(&opened);
        browser.set_sql_callback(move |action| {
            if let SqlAction::OpenInNewTab(sql) = action {
                opened.lock().unwrap_or_else(|p| p.into_inner()).push(sql);
            }
        });
    }
    if !browser.refresh() {
        return Err("object browser refused to load metadata".to_string());
    }
    // FLTK widgets are not `Send`, and this all runs on the UI thread anyway.
    let cache_probe = browser.clone();
    pump_until("object metadata to load", 180, || {
        let cache = cache_probe.object_cache_snapshot();
        cache
            .tables
            .iter()
            .any(|name| name.eq_ignore_ascii_case(PARENT_TABLE))
    })?;
    let cache: ObjectCache = browser.object_cache_snapshot();

    for (name, _) in target.declaration_cases() {
        let hits = search(&cache, name, MAX_OBJECT_SEARCH_HITS);
        if !hits
            .iter()
            .any(|hit| hit.display_name.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "object search did not find {name}; got {:?}",
                hits.iter()
                    .map(|hit| hit.display_name.as_str())
                    .collect::<Vec<_>>()
            ));
        }
    }
    println!("object search found every seeded object");

    let data = IntellisenseData::new();
    for (name, expected_in_source) in target.declaration_cases() {
        opened.lock().unwrap_or_else(|p| p.into_inner()).clear();
        if !browser.open_declaration_for_sql_selection(name, &data) {
            return Err(format!("{name} did not resolve to any object"));
        }
        let opened_probe = Arc::clone(&opened);
        pump_until(&format!("{name} source to open"), 120, || {
            !opened_probe
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        })?;
        let source = opened
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first()
            .cloned()
            .unwrap_or_default();
        if !source.to_uppercase().contains(expected_in_source) {
            return Err(format!(
                "source opened for {name} does not mention it: {}",
                source.chars().take(160).collect::<String>()
            ));
        }
        println!("  {name:<16} opened {} bytes of source", source.len());
    }

    drop(browser);
    // Let the widget's pooled session actually go back before the caller drops
    // the objects it was reading.
    let _ = pump_until("the browser session to be released", 5, || false);
    Ok(())
}

/// The non-blank Operation cells of a rendered plan.
///
/// Only the step list: checking the drawing against values parsed back out of
/// the same drawing would be circular, so the connector assertions run against
/// nodes whose parent links are known instead.
fn plan_step_labels(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| row.first().cloned())
        .filter(|operation| !operation.trim().is_empty())
        .collect()
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
