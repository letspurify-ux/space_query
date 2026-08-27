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
//       source really opens, for every object type each backend supports;
//   (4) that both halves of read-only govern F6 where they should and nowhere
//       else — the tab's READ ONLY pin and the connection's own read-only flag
//       refuse Oracle's `EXPLAIN PLAN` (it writes to `PLAN_TABLE`) and refuse
//       neither family's `EXPLAIN` (it only reports);
//   (5) that F6 takes the text `Ctrl+Enter` would send, on both roads: the two
//       statements read DIFFERENT tables, so the plan itself says which one was
//       explained;
//   (6) that a statement already written as an explain, one ending in a
//       terminator, and one carrying a placeholder all still explain;
//   (7) that a plan says what it cannot see when the requesting tab's own
//       session holds something — and says nothing when it does not. Only a
//       server can show this: the two families track that state differently,
//       and a note that could fire on one and not the other would be the
//       asymmetry it exists to remove.
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
        // Read by the message's KIND, not by its opening words. F6 has two
        // kinds of "no plan" — a refusal this app decided and a failure that
        // happened — and they no longer share a prefix, deliberately: the pane
        // used to announce `Explain plan failed: Explain plan was not run: …`,
        // the app reporting a failure of its own rule. Both arrive as Errors,
        // and the note that says what a plan cannot see arrives as Info, so the
        // kind separates exactly the right things.
        QueryProgress::Message {
            kind: space_query::ui::result_tabs::ResultMessageKind::Error,
            lines,
        } => Some(lines.join(" | ")),
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
    /// Kept so a probe can build a SECOND editor on the same connection — one
    /// whose tab has run nothing, which is the only way to show what a tab with
    /// no session of its own is told.
    shared: Arc<Mutex<DatabaseConnection>>,
}

impl Harness {
    /// A tab of its own on an existing connection, with the callbacks the main
    /// window installs.
    fn for_connection(shared: Arc<Mutex<DatabaseConnection>>) -> Self {
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
        Self {
            editor,
            events,
            done,
            shared,
        }
    }

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

    /// `Ctrl+Enter`'s own path, on whatever the caret or the selection says.
    ///
    /// Its text comes from the same decision F6 takes
    /// (`statement_source_for_single_action`), which is the point of the probe
    /// that uses it: the two used to decide separately and F6 ignored the
    /// selection entirely.
    fn run_at_cursor(&mut self) -> Result<Vec<QueryProgress>, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        self.editor.execute_statement_at_cursor();
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

    /// F6's own path on the text already in the buffer, leaving the caret and
    /// the selection exactly as the caller placed them.
    fn explain_in_place(&mut self) -> Result<space_query::db::QueryResult, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
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

    /// F6's own path, keeping every progress event it produced.
    ///
    /// `explain` below returns only the plan; the note that says what a plan
    /// cannot see travels beside it as its own message, so a probe about the
    /// note has to see them all.
    fn explain_events(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        self.explain(sql)?;
        Ok(self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone())
    }

    /// F6's own path: put the statement in the buffer, then explain it.
    fn explain(&mut self, sql: &str) -> Result<space_query::db::QueryResult, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.editor.set_text(sql);
        self.editor.place_caret_for_probe(0);
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

/// The plan says what it cannot see, when this tab's session holds something.
///
/// The plan is built on the connection's OWN session, so anything living only
/// in the TAB's session is invisible to it. That is not a wart the app can
/// remove — building the plan on the tab's session would leave a MySQL tab
/// looking like it carries a transaction (`EXPLAIN` under `autocommit = 0`
/// opens one) — so the app SAYS it instead, from the state it already tracks.
///
/// Driven on every backend because the two families track that state
/// differently: the MySQL family names the temporary table, Oracle only knows
/// its session holds something a pool setup would not restate. A note that
/// could fire on one family and not the other would be the asymmetry this
/// feature exists to remove, and only a server can show which one fires.
fn verify_a_plan_says_what_it_cannot_see(h: &mut Harness, target: Target) -> Result<(), String> {
    let note_in = |events: &[QueryProgress]| -> Option<String> {
        events.iter().find_map(|event| match progress_inner(event) {
            QueryProgress::Message { lines, .. } => lines
                .iter()
                .find(|line| line.contains("connection's own DB session"))
                .cloned(),
            _ => None,
        })
    };

    // A tab that has run NOTHING has no session of its own, so there is
    // nothing the plan could be missing and nothing is said. A disclaimer on
    // every plan is one the user learns to skip, so the silence is worth
    // proving — and it is proven on a fresh editor, because THIS harness has
    // deliberately run `ALTER SESSION SET ddl_lock_timeout` / `SET SESSION
    // lock_wait_timeout` on its own tab, which is exactly the state the note
    // exists to report.
    {
        let mut fresh = Harness::for_connection(Arc::clone(&h.shared));
        let events = fresh.explain_events(&target.explain_target_sql())?;
        if let Some(note) = note_in(&events) {
            return Err(format!(
                "a plan said what it cannot see for a tab that has run nothing: {note}"
            ));
        }
    }
    println!("  OK  a tab with no session of its own is told nothing");

    if target.is_oracle() {
        // A PL/SQL block leaves state Oracle's model cannot name — package
        // variables, a session setting the pool setup does not restate — so
        // the note falls back to the generic word for it.
        h.run("BEGIN NULL; END;")
            .map_err(|e| format!("run a PL/SQL block on the tab's session: {e}"))?;
        let events = h.explain_events(&target.explain_target_sql())?;
        let note = note_in(&events).ok_or_else(|| {
            "the plan said nothing after this tab's session ran a PL/SQL block".to_string()
        })?;
        if !note.contains("session state") {
            return Err(format!("the note does not name what Oracle knows: {note}"));
        }
        println!(
            "  OK  after a PL/SQL block, the plan says it does not see this tab's session state"
        );
        let _ = h.run("ROLLBACK");
    }

    // The LOUD case, and it must read the same on all four backends: a table
    // that exists ONLY on this tab's session, so the plan cannot even resolve
    // the name. Each family has one and each spells it differently, but the
    // note must name it with the same words.
    //
    // Oracle's had no words at all. Its residue model set nothing for a
    // `CREATE PRIVATE TEMPORARY TABLE`, so a tab that made one was told
    // NOTHING when F6 answered `ORA-00942` for a table only it has — while the
    // MySQL half of the same sentence has always worked. That asymmetry is the
    // one this whole note exists to remove.
    let (create, probe_table, drop) = if target.is_oracle() {
        (
            "CREATE PRIVATE TEMPORARY TABLE ora$ptt_note_probe (id NUMBER) \
             ON COMMIT PRESERVE DEFINITION",
            "ora$ptt_note_probe",
            "DROP TABLE ora$ptt_note_probe",
        )
    } else {
        (
            "CREATE TEMPORARY TABLE oqt_plan_note_probe (id INT)",
            "oqt_plan_note_probe",
            "DROP TEMPORARY TABLE IF EXISTS oqt_plan_note_probe",
        )
    };
    h.run(create)
        .map_err(|e| format!("create a session-only table on the tab's session: {e}"))?;
    let outcome = h.explain(&format!("SELECT * FROM {probe_table}"));
    let _ = h.run(drop);
    match outcome {
        Ok(_) => {
            Err("the plan resolved a table that exists only on this tab's session".to_string())
        }
        Err(message) if message.contains("connection's own DB session") => {
            if !message.contains("temporary tables") {
                return Err(format!(
                    "the note does not name the temporary table: {message}"
                ));
            }
            println!("  OK  a plan that cannot resolve this tab's temporary table says why");
            Ok(())
        }
        Err(message) => Err(format!(
            "the plan failed on this tab's temporary table without saying why: {message}"
        )),
    }
}

/// F6 explains the statement `Ctrl+Enter` would run — both roads, on a server.
///
/// Which text a single-statement action takes used to be decided twice:
/// execution preferred the SELECTION, F6 ignored it and always took the
/// statement at the caret. So a user who selected one query and pressed F6 got
/// the plan of whichever statement the caret happened to sit in. One decision
/// (`SqlEditorWidget::statement_source_for_single_action`) now answers both,
/// and this drives both of its roads through the real editor: the two
/// statements read DIFFERENT tables, so the plan itself says which one was
/// explained.
fn verify_explain_takes_the_text_execution_would(
    h: &mut Harness,
    target: Target,
) -> Result<(), String> {
    let at_cursor = format!("SELECT DNAME FROM {PARENT_TABLE}");
    let selected = format!("SELECT EMPNO FROM {CHILD_TABLE}");
    let script = format!("{at_cursor};\n{selected}");

    let plan_names = |plan: &space_query::db::QueryResult| -> String {
        plan.rows
            .iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>()
            .join(" ")
            .to_uppercase()
    };
    // EVERY column the statement came back with, not just the first: an
    // editable SELECT carries an injected ROWID column ahead of the user's own
    // (`maybe_inject_rowid_for_editing`), and which column leads is that
    // feature's business rather than this probe's.
    let columns = |events: &[QueryProgress]| -> String {
        events
            .iter()
            .find_map(|event| match progress_inner(event) {
                QueryProgress::StatementFinished { result, .. } => Some(
                    result
                        .columns
                        .iter()
                        .map(|column| column.name.to_uppercase())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                _ => None,
            })
            .unwrap_or_default()
    };

    // Road 1: no selection, caret in the FIRST statement.
    h.editor.set_text(&script);
    h.editor.place_caret_for_probe(1);
    let ran = h
        .run_at_cursor()
        .map_err(|e| format!("run at caret: {e}"))?;
    let ran_columns = columns(&ran);
    if !ran_columns.contains("DNAME") || ran_columns.contains("EMPNO") {
        return Err(format!(
            "the caret was in `{at_cursor}` but execution ran something else (columns {ran_columns})"
        ));
    }
    h.editor.place_caret_for_probe(1);
    let plan = h
        .explain_in_place()
        .map_err(|e| format!("explain at caret: {e}"))?;
    if !plan_names(&plan).contains(PARENT_TABLE) || plan_names(&plan).contains(CHILD_TABLE) {
        return Err(format!(
            "the caret was in `{at_cursor}` but the plan names other tables: {}",
            plan_names(&plan)
        ));
    }
    println!("  OK  with no selection, F6 explains the statement the caret is in");

    // Road 2: the SECOND statement selected. Execution has always preferred a
    // selection; F6 must now agree with it.
    let start = i32::try_from(script.find(&selected).unwrap_or(0))
        .map_err(|_| "selection offset does not fit".to_string())?;
    let end = start
        + i32::try_from(selected.len()).map_err(|_| "selection length does not fit".to_string())?;
    h.editor.select_for_probe(start, end);
    let ran = h
        .run_at_cursor()
        .map_err(|e| format!("run on a selection: {e}"))?;
    let ran_columns = columns(&ran);
    if !ran_columns.contains("EMPNO") || ran_columns.contains("DNAME") {
        return Err(format!(
            "execution did not run the selected statement (columns {ran_columns})"
        ));
    }
    h.editor.select_for_probe(start, end);
    let plan = h
        .explain_in_place()
        .map_err(|e| format!("explain on a selection: {e}"))?;
    if !plan_names(&plan).contains(CHILD_TABLE) || plan_names(&plan).contains(PARENT_TABLE) {
        return Err(format!(
            "F6 did not explain the SELECTED statement: {}",
            plan_names(&plan)
        ));
    }
    println!("  OK  with a selection, F6 explains the same statement execution runs");

    // A selection holding MORE than one statement is refused rather than
    // narrowed to its first. That refusal never reaches the server, so it is
    // pinned where it lives:
    // `a_selection_is_explained_only_when_it_holds_one_statement`.
    h.editor.place_caret_for_probe(0);
    let _ = target;
    Ok(())
}

/// An explain builds a plan and changes nothing, on every backend.
///
/// Two halves, and the second is why this exists:
///
/// * explaining a statement that WOULD write must still produce a plan and must
///   not write. This is the ordinary case on all four backends, and the one the
///   guard below must not break;
/// * an explain that would RUN what it explains must never be sent. On the
///   MySQL family `EXPLAIN ANALYZE <statement>` executes it, and an explain runs
///   on the connection's OWN session — which no query tab owns, so nothing in
///   the transaction model would ever commit or roll back what it changed
///   there. Neither read-only gate catches it on its own: most connections have
///   neither flag set, and this family's READ ONLY pin is a characteristic of
///   the TAB's session, which the explain does not run on.
///
/// Server versions differ on WHICH statements `EXPLAIN ANALYZE` will run —
/// measured on MySQL 8.0.46, DML comes back `<not executable by iterator
/// executor>`; MySQL 8.3 extended the iterator executor to cover it — so the
/// probe asserts the app's answer rather than the server's, which is exactly
/// why the app has one.
fn verify_an_explain_only_builds_a_plan(h: &mut Harness, target: Target) -> Result<(), String> {
    let marker = 4242;
    let planted = format!("INSERT INTO {PARENT_TABLE} VALUES ({marker}, 'EXPLAINED')");
    let rows_with_marker = |h: &mut Harness| -> Result<i64, String> {
        let events = h.run(&format!(
            "SELECT COUNT(*) FROM {PARENT_TABLE} WHERE DEPTNO = {marker}"
        ))?;
        // A grid's rows travel as their own event: `StatementFinished` carries
        // the statement's outcome, not the rows a still-streaming grid holds.
        events
            .iter()
            .find_map(|event| match progress_inner(event) {
                QueryProgress::Rows { rows, .. } => rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(|value| value.trim().parse::<i64>().ok()),
                _ => None,
            })
            .ok_or_else(|| "the marker count returned no row".to_string())
    };

    // Explaining a write is ordinary work, and it stays ordinary.
    let plan = h
        .explain(&planted)
        .map_err(|e| format!("explain of a statement that would write: {e}"))?;
    if plan.rows.is_empty() {
        return Err("explaining a write produced no plan".to_string());
    }
    if rows_with_marker(h)? != 0 {
        return Err("explaining an INSERT inserted the row".to_string());
    }
    println!("  OK  explaining a write builds a plan and writes nothing");

    if target.is_oracle() {
        // Oracle has no spelling that runs what it explains: `EXPLAIN PLAN`
        // only parses. The `PLAN_TABLE` row it writes is this call's own, and
        // the rollback that takes it back is checked above.
        return Ok(());
    }

    // Each product's OWN spelling of the explain that runs what it explains,
    // and the refusal must name that one. MariaDB rejects `EXPLAIN ANALYZE`
    // outright and writes `ANALYZE <statement>`, so a refusal that said
    // `EXPLAIN ANALYZE` there described a statement the server does not have
    // and the user did not type.
    let spelling = match target {
        Target::MariaDb => "ANALYZE",
        _ => "EXPLAIN ANALYZE",
    };
    let would_run = format!("{spelling} {planted}");
    match h.explain(&would_run) {
        Ok(_) => Err(format!(
            "`{would_run}` was sent: it runs the statement it explains, on a session no tab owns"
        )),
        Err(message)
            if message.contains(&format!("{spelling} executes the statement it explains")) =>
        {
            if rows_with_marker(h)? != 0 {
                return Err("the refused explain still inserted the row".to_string());
            }
            if target == Target::MariaDb && message.contains("EXPLAIN ANALYZE") {
                return Err(format!(
                    "MariaDB was told about `EXPLAIN ANALYZE`, which it rejects: {message}"
                ));
            }
            println!("  OK  an explain that would RUN what it explains is never sent");
            println!("  OK  and the refusal names it in this product's own spelling");
            Ok(())
        }
        Err(message) => Err(format!(
            "`{would_run}` was refused, but not as a statement that would run: {message}"
        )),
    }
}

/// A statement with NO execution plan is answered, not wrapped and sent.
///
/// F6 wraps whatever it is handed, so each of these went to a server and every
/// backend answered the one keystroke with its own complaint. `ANALYZE TABLE`
/// was worse than an unhelpful error and only a server shows why: MySQL reads
/// the wrapped `EXPLAIN ANALYZE TABLE t` as an executing explain of the
/// `TABLE t` QUERY, so F6 on a maintenance statement drew a real measured plan
/// — of a full scan of the table — while MariaDB answered `ERROR 1064` and
/// Oracle a parse error. One keystroke, three different wrong answers.
fn verify_a_statement_with_no_plan_is_answered_not_sent(
    h: &mut Harness,
    target: Target,
) -> Result<(), String> {
    let mut cases: Vec<(String, &str)> = vec![
        (
            format!("ANALYZE TABLE {PARENT_TABLE}"),
            "a table maintenance statement",
        ),
        ("COMMIT".to_string(), "a transaction control statement"),
    ];
    // The probe routine takes one argument on every backend, so the call is
    // written with it: a refusal that only happened because the CALL was
    // malformed would prove nothing.
    if target.is_oracle() {
        cases.push((format!("BEGIN {PROC_NAME}(1); END;"), "a PL/SQL block"));
        cases.push((
            format!("ANALYZE TABLE {PARENT_TABLE} COMPUTE STATISTICS"),
            "a table maintenance statement",
        ));
        // MEASURED (Oracle 23.26): ORA-00905 to both wrapped forms. `SHOW` is
        // SQL*Plus's word and begins no server statement; `LOCK TABLE` is DML
        // to the classifier — fail-open — so only the grammar read refuses it.
        // The refusal is also what keeps the probe from really locking the
        // table: nothing is sent.
        cases.push((
            "SHOW PARAMETER open_cursors".to_string(),
            "a SHOW statement",
        ));
        cases.push((
            format!("LOCK TABLE {PARENT_TABLE} IN EXCLUSIVE MODE"),
            "a lock statement",
        ));
    } else {
        cases.push((format!("CALL {PROC_NAME}(1)"), "a routine call"));
        // MEASURED (MySQL 8.0.46, MariaDB 12.2.2): ERROR 1064 to every
        // wrapped `SHOW` — a `SHOW` classifies `SelectLike` (rows really come
        // back), the one kind the gate used to trust as certainly plannable,
        // so it was wrapped and SENT. `SHOW INDEX FROM t` and not one of the
        // spellings the splitter reads as a tool command, so this really
        // reaches the backend gate.
        cases.push((
            format!("SHOW INDEX FROM {PARENT_TABLE}"),
            "a SHOW statement",
        ));
        // MEASURED: `EXPLAIN CHECK TABLE t` is ERROR 1064 on both products;
        // the statement leaked through the kind match as fail-open DDL.
        cases.push((
            format!("CHECK TABLE {PARENT_TABLE}"),
            "a table maintenance statement",
        ));
    }

    for (sql, subject) in cases {
        let expected = format!("There is no execution plan for {subject}.");
        match h.explain(&sql) {
            Ok(plan) => {
                return Err(format!(
                    "`{sql}` produced a plan of {} rows; it has none to produce",
                    plan.rows.len()
                ))
            }
            Err(message) if message.contains(&expected) => {}
            Err(message) => {
                return Err(format!(
                    "`{sql}` was answered `{message}`, not `{expected}`"
                ))
            }
        }
    }
    println!("  OK  a statement with no execution plan is answered, not sent");

    // ... and the ordinary case is untouched, which is what keeps the gate
    // from having swallowed the feature.
    let plan = h
        .explain(&format!("SELECT * FROM {PARENT_TABLE}"))
        .map_err(|e| format!("an ordinary SELECT stopped being explainable: {e}"))?;
    if plan.rows.is_empty() {
        return Err("an ordinary SELECT produced no plan".to_string());
    }
    println!("  OK  and an ordinary statement still explains");
    Ok(())
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

/// Server cursors this session currently holds open.
///
/// `v$sesstat`, not `v$open_cursor`: a cursor a thin call left behind carries no
/// `sql_text` and does not show in the view at all, which is exactly why this
/// class of leak has shipped twice. The stat counts them.
fn live_session_open_cursors(shared: &Arc<Mutex<DatabaseConnection>>) -> Result<i64, String> {
    const SQL: &str = "SELECT TO_CHAR(st.value) FROM v$sesstat st, v$statname sn \
                       WHERE st.statistic# = sn.statistic# \
                       AND sn.name = 'opened cursors current' \
                       AND st.sid = SYS_CONTEXT('USERENV', 'SID')";
    let db_conn = {
        let guard = shared.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .get_db_connection()
            .ok_or_else(|| "no live connection to probe".to_string())?
    };
    let text = match db_conn {
        DbConnection::Oracle(conn) => conn
            .query_row_as::<String>(SQL, &[])
            .map_err(|err| format!("live open-cursor probe (oci): {err}"))?,
        DbConnection::OracleThin(session) => {
            let mut session = session
                .lock()
                .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
            DatabaseConnection::oracle_thin_select_one_text_for_test(&mut session, SQL)
                .map_err(|err| format!("live open-cursor probe (thin): {err}"))?
                .unwrap_or_default()
        }
        DbConnection::MySQL { .. } => {
            return Err("live open-cursor probe is Oracle-only".to_string())
        }
    };
    text.trim()
        .parse::<i64>()
        .map_err(|err| format!("live open-cursor probe returned {text:?}: {err}"))
}

/// An explain must not leave a server cursor behind, on EITHER Oracle driver.
///
/// The thin driver hands the server cursor back inside the statement result and
/// closes nothing itself, so a call that dropped the result dropped the cursor
/// with it. F6's `EXPLAIN PLAN` did exactly that, on the connection's OWN
/// session — which is not pooled, so `reset_before_reuse`'s sweep never runs on
/// it — and one cursor accumulated per F6 for the life of the connection,
/// ending in `ORA-01000`. Measured before the fix: 12 explains, +12 open
/// cursors, exactly one per call.
///
/// Run on BOTH drivers because the answer must be the same on both, and it is:
/// OCI's `Statement` closes when it goes out of scope, and ODPI-C's own
/// statement cache does not hold these open either (measured: delta 0 across
/// the same 12 repeats, with each explain carrying a unique `STATEMENT_ID`).
/// A driver that starts differing from its twin here is the divergence this
/// app's Oracle work exists to remove, and only a server can say so.
///
/// Measured as a DELTA across repeats rather than as an absolute, because the
/// app's own metadata work moves the number: a leak GROWS with every F6, one
/// cursor per call.
fn verify_an_explain_leaves_no_open_cursor(
    h: &mut Harness,
    shared: &Arc<Mutex<DatabaseConnection>>,
    target: Target,
) -> Result<(), String> {
    const REPEATS: usize = 12;
    /// Half the repeat count, so a leak and the noise are far apart: a leak is
    /// one cursor per explain (12), while Oracle's session cursor cache can
    /// hold only a handful — the plan read-back is the same SQL every time, and
    /// each explain's own `EXPLAIN PLAN SET STATEMENT_ID = '…'` text is unique
    /// and therefore never cached. Measured on both drivers with the fix in
    /// place: delta 0.
    const LEAKED_CURSOR_THRESHOLD: i64 = 6;

    // One explain first, so anything the FIRST one caches (a parsed statement,
    // a described column set) is already paid for and out of the delta.
    h.explain(&target.explain_target_sql())
        .map_err(|e| format!("warm-up explain: {e}"))?;
    let before = live_session_open_cursors(shared)?;
    for index in 0..REPEATS {
        h.explain(&target.explain_target_sql())
            .map_err(|e| format!("explain #{index}: {e}"))?;
    }
    let after = live_session_open_cursors(shared)?;

    let grew = after - before;
    println!("      {REPEATS} explains moved open cursors {before} -> {after} (delta {grew})");
    if grew >= LEAKED_CURSOR_THRESHOLD {
        return Err(format!(
            "{REPEATS} explains left {grew} more open cursor(s) on the connection's own session \
             ({before} -> {after}): the statement result was dropped with its cursor inside"
        ));
    }
    println!("  OK  repeated explains leave no cursor behind on the connection's own session");
    Ok(())
}

/// MariaDB's own executing explain reaches the server as the user wrote it.
///
/// MariaDB rejects `EXPLAIN ANALYZE` outright and spells its executing explain
/// `ANALYZE <statement>`. The app's builder knew only MySQL's spelling, so it
/// wrapped MariaDB's into an `EXPLAIN ANALYZE …` the server refuses — a MySQL
/// user could read a measured plan through F6 and a MariaDB user could not.
///
/// The write form must still be refused, and refused by the app rather than by
/// the server: `ANALYZE UPDATE …` really writes on MariaDB (measured), and it
/// would write on the connection's OWN session, which no tab owns.
fn verify_mariadb_explains_its_own_analyze_spelling(h: &mut Harness) -> Result<(), String> {
    let marker = 4343;
    let planted = format!("INSERT INTO {PARENT_TABLE} VALUES ({marker}, 'ANALYZED')");
    let rows_with_marker = |h: &mut Harness| -> Result<i64, String> {
        let events = h.run(&format!(
            "SELECT COUNT(*) FROM {PARENT_TABLE} WHERE DEPTNO = {marker}"
        ))?;
        events
            .iter()
            .find_map(|event| match progress_inner(event) {
                QueryProgress::Rows { rows, .. } => rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(|value| value.trim().parse::<i64>().ok()),
                _ => None,
            })
            .ok_or_else(|| "the marker count returned no row".to_string())
    };

    let read = format!("ANALYZE {}", Target::MariaDb.explain_target_sql());
    let plan = h
        .explain(&read)
        .map_err(|e| format!("MariaDB's own executing explain of a read: {e}"))?;
    if plan.rows.is_empty() {
        return Err("MariaDB's own executing explain produced no plan".to_string());
    }
    println!("  OK  MariaDB's `ANALYZE <select>` reaches the server as written");

    let would_write = format!("ANALYZE {planted}");
    match h.explain(&would_write) {
        Ok(_) => Err(format!(
            "`{would_write}` was sent: it runs the statement it explains, on a session no tab owns"
        )),
        // CHANGED, with its reason: this asserted the literal `EXPLAIN ANALYZE`
        // — the spelling MariaDB REJECTS — on the one probe whose whole subject
        // is MariaDB's own spelling. The refusal must name the statement this
        // server really has and the user really typed.
        Err(message) if message.contains("ANALYZE executes the statement it explains") => {
            if message.contains("EXPLAIN ANALYZE") {
                return Err(format!(
                    "MariaDB was told about `EXPLAIN ANALYZE`, which it rejects: {message}"
                ));
            }
            if rows_with_marker(h)? != 0 {
                return Err("the refused MariaDB analyze still inserted the row".to_string());
            }
            println!("  OK  MariaDB's `ANALYZE <write>` is refused before it is sent");
            println!("  OK  and it is refused in MariaDB's own spelling");
            Ok(())
        }
        Err(message) => Err(format!(
            "`{would_write}` was refused, but not as a statement that would run: {message}"
        )),
    }
}

/// F6 on a statement written inside MariaDB's `SET STATEMENT … FOR` wrapper.
///
/// Only this server settles the premise: `EXPLAIN SET STATEMENT … FOR SELECT …`
/// — what wrapping the whole wrapper produced — is ERROR 1064, while
/// `SET STATEMENT … FOR EXPLAIN SELECT …` answers with a real plan. So the one
/// product with a statement-scoped way to bound an explain could not explain
/// anything written with it, and MySQL — which rejects the wrapper outright —
/// answered the same keystroke differently again.
fn verify_mariadb_explains_inside_its_set_statement_wrapper(h: &mut Harness) -> Result<(), String> {
    // A plain statement inside the wrapper: the app rebuilds it with the
    // explain INSIDE, and the server answers with the inner statement's plan.
    let wrapped = format!("SET STATEMENT max_statement_time=10 FOR SELECT * FROM {PARENT_TABLE}");
    let plan = h
        .explain(&wrapped)
        .map_err(|e| format!("explain of a SET STATEMENT wrapper: {e}"))?;
    if plan.rows.is_empty() {
        return Err("a SET STATEMENT wrapper produced no plan".to_string());
    }
    if !plan
        .columns
        .iter()
        .any(|column| column.name.eq_ignore_ascii_case("id"))
    {
        return Err(format!(
            "a SET STATEMENT wrapper's plan is missing the server's own columns: {:?}",
            plan.columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>()
        ));
    }
    println!("  OK  a statement inside `SET STATEMENT … FOR` explains (the explain goes inside)");

    // One that already carries its own executing explain passes through whole
    // — `EXPLAIN ANALYZE` is the spelling this server rejects, so a rewrap in
    // either direction would be a syntax error.
    let executing =
        format!("SET STATEMENT max_statement_time=10 FOR ANALYZE SELECT * FROM {PARENT_TABLE}");
    let measured = h
        .explain(&executing)
        .map_err(|e| format!("a wrapped executing explain of a read: {e}"))?;
    if measured.rows.is_empty() {
        return Err("a wrapped `ANALYZE <select>` produced no plan".to_string());
    }
    println!("  OK  a wrapped `ANALYZE <select>` reaches the server as written");

    // ... and a wrapped executing explain of a WRITE is refused before it is
    // sent, exactly as the bare spelling is: the wrapper scopes variables, it
    // does not change what the statement RUNS.
    let would_write = format!(
        "SET STATEMENT max_statement_time=10 FOR ANALYZE INSERT INTO {PARENT_TABLE} VALUES (4344, 'WRAPPED')"
    );
    match h.explain(&would_write) {
        Ok(_) => Err(format!(
            "`{would_write}` was sent: it runs the statement it explains, on a session no tab owns"
        )),
        Err(message) if message.contains("ANALYZE executes the statement it explains") => {
            println!("  OK  a wrapped `ANALYZE <write>` is refused before it is sent");
            Ok(())
        }
        Err(message) => Err(format!(
            "`{would_write}` was refused, but not as a statement that would run: {message}"
        )),
    }
}

/// The gate reads executable comments the way the server executes them.
///
/// Only a server settles the premise, and both were measured: MySQL 8.0.46
/// ran `EXPLAIN /*! ANALYZE */ SELECT SLEEP(2)` for the full two seconds —
/// the comment IS the ANALYZE — and MySQL ≥ 8.3's iterator executor runs DML
/// under `EXPLAIN ANALYZE`, so a gate that parsed the raw bytes (where
/// `/*! … */` is a comment) let `EXPLAIN /*! ANALYZE */ UPDATE …` reach the
/// connection's own session. MariaDB rejects that spelling (1064) but
/// executes `/*! ANALYZE */ <statement>` — measured: an UPDATE written that
/// way really wrote.
fn verify_the_gate_reads_executable_comments_like_the_server(
    h: &mut Harness,
    target: Target,
) -> Result<(), String> {
    // An executing explain of a WRITE hidden in an executable comment is
    // refused before it is sent.
    let would_write = format!("EXPLAIN /*! ANALYZE */ UPDATE {PARENT_TABLE} SET DNAME = DNAME");
    match h.explain(&would_write) {
        Ok(_) => {
            return Err(format!(
                "`{would_write}` was sent: the server reads it as EXPLAIN ANALYZE UPDATE"
            ))
        }
        Err(message) if message.contains("executes the statement it explains") => {
            println!("  OK  a comment-hidden executing explain of a write is refused");
        }
        Err(message) => {
            return Err(format!(
                "`{would_write}` was refused, but not as a statement that would run: {message}"
            ))
        }
    }
    // ... and one of a READ reaches the server as the user's own explain, in
    // the spelling THIS product executes.
    let read = match target {
        Target::MySql => format!("EXPLAIN /*! ANALYZE */ SELECT * FROM {PARENT_TABLE}"),
        _ => format!("/*! ANALYZE */ SELECT * FROM {PARENT_TABLE}"),
    };
    let plan = h
        .explain(&read)
        .map_err(|e| format!("a comment-hidden executing explain of a read: {e}"))?;
    if plan.rows.is_empty() {
        return Err("a comment-hidden executing explain of a read produced no plan".to_string());
    }
    println!("  OK  a comment-hidden executing explain of a read answers with the server's plan");
    Ok(())
}

/// `SHOW EXPLAIN FOR <id>` / `SHOW ANALYZE FOR <id>`: MariaDB's own explains
/// of a RUNNING statement, refused on the product that does not have them.
///
/// Only a server settles both halves: MariaDB 12.2.2 answers real plan rows
/// for a running statement's id and ERROR 1094 `Unknown thread id` for a bad
/// one — while MySQL 8.0.46 answers ERROR 1064, because there the same intent
/// is spelled `EXPLAIN FOR CONNECTION <id>` (which passes through on its
/// leading word). The app used to refuse the SHOW spellings everywhere as
/// "a SHOW statement" — a sentence that is literally false for a statement
/// whose whole output IS an execution plan.
fn verify_show_spelled_explain_per_product(h: &mut Harness, target: Target) -> Result<(), String> {
    if target == Target::MySql {
        match h.explain("SHOW EXPLAIN FOR 1") {
            Ok(_) => return Err("MySQL sent `SHOW EXPLAIN FOR 1`".to_string()),
            Err(message) if message.contains("There is no execution plan for a SHOW statement") => {
                println!("  OK  MySQL keeps the SHOW refusal (its spelling is FOR CONNECTION)");
                return Ok(());
            }
            Err(message) => {
                return Err(format!(
                    "MySQL refused `SHOW EXPLAIN FOR 1`, but not as a SHOW statement: {message}"
                ))
            }
        }
    }

    use mysql::prelude::Queryable;
    // A raw side connection running a long statement, so the id names work
    // that is really in flight when F6's statement executes.
    let opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some("127.0.0.1"))
        .tcp_port(3306)
        .user(Some("root"))
        .pass(Some("password"))
        .db_name(Some("query_tool_test"));
    let mut sleeper = mysql::Conn::new(opts).map_err(|e| format!("side connection: {e}"))?;
    let sleeper_id = sleeper.connection_id();
    let sleeper_thread = std::thread::spawn(move || {
        // Ended by the KILL QUERY below; the bound is only a backstop.
        let _ = sleeper.query_drop("SELECT SLEEP(60)");
    });
    // Give the sleeper time to be ON the server before asking about it.
    std::thread::sleep(Duration::from_millis(500));

    let outcome = (|| -> Result<(), String> {
        let plan = h
            .explain(&format!("SHOW EXPLAIN FOR {sleeper_id}"))
            .map_err(|e| format!("SHOW EXPLAIN FOR a running statement: {e}"))?;
        if plan.rows.is_empty() {
            return Err("SHOW EXPLAIN FOR answered no rows for a running statement".to_string());
        }
        if !plan
            .columns
            .iter()
            .any(|column| column.name.eq_ignore_ascii_case("id"))
        {
            return Err(format!(
                "SHOW EXPLAIN FOR is missing the server's own plan columns: {:?}",
                plan.columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
            ));
        }
        println!("  OK  `SHOW EXPLAIN FOR <running id>` answers with the running plan");

        // A bad id must come back as the SERVER's failure — proof the
        // statement was SENT rather than refused by the app.
        match h.explain("SHOW EXPLAIN FOR 999999") {
            Ok(_) => Err("`SHOW EXPLAIN FOR 999999` answered a plan".to_string()),
            Err(message) if message.contains("Unknown thread id") => {
                println!("  OK  a bad id fails with the server's own sentence (it was sent)");
                Ok(())
            }
            Err(message) => Err(format!(
                "`SHOW EXPLAIN FOR 999999` failed, but not with the server's answer: {message}"
            )),
        }
    })();

    // End the sleeper whatever the outcome, so the run leaves no session
    // holding a 60-second call.
    let kill_opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some("127.0.0.1"))
        .tcp_port(3306)
        .user(Some("root"))
        .pass(Some("password"))
        .db_name(Some("query_tool_test"));
    if let Ok(mut killer) = mysql::Conn::new(kill_opts) {
        let _ = killer.query_drop(format!("KILL QUERY {sleeper_id}"));
    }
    let _ = sleeper_thread.join();
    outcome
}

/// A plan the SERVER draws is readable in the grid, on both products.
///
/// Only a server settles the premise: these formats answer with ONE column
/// holding the whole plan as one string, newlines and all. Passed through as a
/// single cell the grid showed the first line of it and the rest lived behind a
/// double-click, while the same keystroke on MariaDB's tabular `ANALYZE SELECT`
/// produced a readable table — so how legible a plan was depended on which
/// product answered.
///
/// Each product is asked in a spelling it really has: `FORMAT = TREE` is
/// MySQL's and MariaDB rejects it; `FORMAT = JSON` is the one both draw. Both
/// are plan-only, so nothing here runs what it explains.
fn verify_a_server_drawn_plan_is_readable(h: &mut Harness, target: Target) -> Result<(), String> {
    let format = match target {
        Target::MySql => "TREE",
        _ => "JSON",
    };
    let sql = format!("EXPLAIN FORMAT={format} {}", target.explain_target_sql());
    let plan = h
        .explain(&sql)
        .map_err(|e| format!("a server-drawn plan (`{sql}`): {e}"))?;
    if plan.columns.len() != 1 {
        return Err(format!(
            "`{sql}` came back with {} columns; this probe is about the one-column form",
            plan.columns.len()
        ));
    }
    if plan.rows.len() < 2 {
        return Err(format!(
            "`{sql}` produced {} grid row(s): the server draws it over several lines, and each \
             must be a row",
            plan.rows.len()
        ));
    }
    if let Some(row) = plan
        .rows
        .iter()
        .find(|row| row.iter().any(|value| value.contains('\n')))
    {
        return Err(format!(
            "a grid row still holds more than one line of the plan: {row:?}"
        ));
    }
    println!("  OK  a plan the server draws itself is one grid row per line");
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

    let mut h = Harness::for_connection(Arc::clone(&shared));

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

    // A statement the user already wrote as an explain is explained, not
    // wrapped again. The MySQL family has always passed its own `EXPLAIN`
    // through; Oracle wrapped it and the server rejected the double wrap, so
    // F6 answered a raw syntax error for a statement whose plan was plainly
    // being asked for.
    let already = if target.is_oracle() {
        format!("EXPLAIN PLAN FOR {}", target.explain_target_sql())
    } else {
        format!("EXPLAIN {}", target.explain_target_sql())
    };
    let already_plan = h
        .explain(&already)
        .map_err(|e| format!("explain of a statement that is already an explain: {e}"))?;
    if already_plan.rows.is_empty() {
        return Err("a statement that is already an explain produced no plan".to_string());
    }
    println!("  OK  a statement already written as an explain is explained, not wrapped again");

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
                // ... and it says WHY an execution plan is a write here. The
                // shared read-only wording describes the statement that was
                // refused, and on Oracle that statement is the
                // `EXPLAIN PLAN … FOR` the app built — so the user asked for
                // the plan of a `SELECT` and read "blocks non-query
                // statements", about a statement they had not typed, while the
                // same keystroke simply worked on the other family.
                if !message.contains("PLAN_TABLE") {
                    return Err(format!(
                        "the refusal never says why a plan is a write here: {message}"
                    ));
                }
                println!("  OK  a Read only tab refuses F6 with the same message as a write");
                println!("  OK  and the refusal says the plan itself is the write");
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

    // ---- an explain only ever builds a plan -------------------------------
    verify_an_explain_only_builds_a_plan(&mut h, target)?;

    // ---- ... and a statement with no plan is answered, not sent -----------
    verify_a_statement_with_no_plan_is_answered_not_sent(&mut h, target)?;

    // ---- ... and leaves no server cursor behind ---------------------------
    // Both Oracle drivers: thin is where the defect was — it leaves closing the
    // statement's cursor to the caller, and F6 ran on the connection's own,
    // non-pooled session where nothing sweeps up after it — and OCI is what
    // says the two drivers now answer alike.
    if target.is_oracle() {
        verify_an_explain_leaves_no_open_cursor(&mut h, &shared, target)?;
    }

    // ---- each family's own explain spelling reaches the server ------------
    if target == Target::MariaDb {
        verify_mariadb_explains_its_own_analyze_spelling(&mut h)?;
        verify_mariadb_explains_inside_its_set_statement_wrapper(&mut h)?;
    }

    // ---- the gate reads the text the way the server does ------------------
    if !target.is_oracle() {
        verify_the_gate_reads_executable_comments_like_the_server(&mut h, target)?;
        verify_show_spelled_explain_per_product(&mut h, target)?;
    }

    // ---- ... and a plan the server draws itself stays readable ------------
    // Oracle has no such form: its plan comes back as PLAN_TABLE ROWS, which
    // is what the tree above is built from.
    if !target.is_oracle() {
        verify_a_server_drawn_plan_is_readable(&mut h, target)?;
    }

    // ---- F6 explains what Ctrl+Enter runs ---------------------------------
    verify_explain_takes_the_text_execution_would(&mut h, target)?;

    // ---- and it says what it cannot see -----------------------------------
    verify_a_plan_says_what_it_cannot_see(&mut h, target)?;

    // ---- the CONNECTION's read-only flag governs F6 too -------------------
    // The second half of "would this write be refused". `EXPLAIN PLAN … FOR`
    // inserts into PLAN_TABLE, and `docs/session.md` says a connection the user
    // marked read-only makes F6 unavailable on Oracle — it did not, because the
    // explain path asked the tab's pin and never the connection's flag. On the
    // MySQL family `EXPLAIN` is a read, so the same flag must NOT block it: a
    // read-only connection is where reading a plan matters most.
    verify_read_only_connection_governs_explain(target)?;

    // ---- placeholders --------------------------------------------------
    // Oracle: `EXPLAIN PLAN` only PARSES the statement it explains, so its
    // placeholders never need values — the server reports no binds for them and
    // runs it unbound. Nothing is declared or prompted for here, and that is
    // the point: F6 on a statement carrying a placeholder must simply work.
    // (The MySQL family's half is the prompt, which substitutes the answers
    // into the text; a modal cannot be driven from here, so
    // `only_the_backend_whose_statement_needs_the_answers_prompts_for_them`
    // pins which family asks.)
    if target.is_oracle() {
        let with_placeholder = h
            .explain(&format!(
                "SELECT DNAME FROM {PARENT_TABLE} WHERE DEPTNO = :oqt_plan_bind"
            ))
            .map_err(|e| format!("explain of a statement with a placeholder: {e}"))?;
        if with_placeholder.rows.is_empty() {
            return Err("a statement with a placeholder produced no plan".to_string());
        }
        println!("  OK  a statement with a placeholder explains with nothing bound");
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

/// F6 on a connection the user marked read-only.
///
/// A connection of its own, because the flag is a property of the CONNECTION
/// and this must be the same flag the connection dialog sets — not a value
/// poked into a live runtime.
fn verify_read_only_connection_governs_explain(target: Target) -> Result<(), String> {
    let mut info = target.connection_info();
    info.read_only = true;
    let mut connection = DatabaseConnection::new();
    connection
        .connect(info)
        .map_err(|e| format!("connect read-only: {e}"))?;
    let shared: Arc<Mutex<DatabaseConnection>> = Arc::new(Mutex::new(connection));

    let mut guarded = Harness::for_connection(Arc::clone(&shared));

    let attempt = guarded.explain(&target.explain_target_sql());
    if target.is_oracle() {
        match attempt {
            Ok(_) => {
                return Err(
                    "a read-only connection explained anyway: EXPLAIN PLAN writes to PLAN_TABLE"
                        .to_string(),
                )
            }
            Err(message) if message.to_lowercase().contains("is read-only") => {
                if !message.contains("PLAN_TABLE") {
                    return Err(format!(
                        "the refusal never says why a plan is a write here: {message}"
                    ));
                }
                println!("  OK  a read-only connection refuses F6 and says which connection it is");
                println!("  OK  and the refusal says the plan itself is the write");
            }
            Err(message) => {
                return Err(format!(
                "a read-only connection refused F6, but not as a read-only connection: {message}"
            ))
            }
        }
    } else {
        attempt.map_err(|e| {
            format!("a read-only connection blocked a MySQL-family EXPLAIN, which is a read: {e}")
        })?;
        println!("  OK  a read-only connection still explains (EXPLAIN is a read here)");
    }

    drop(guarded);
    let _ = pump_until("the read-only probe connection to settle", 2, || false);
    shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .disconnect();
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
