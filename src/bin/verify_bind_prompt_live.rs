#![allow(clippy::cargo, clippy::pedantic)]

// Live verification of the bind-parameter prompt (item 31) on every supported
// backend: Oracle Thin, Oracle OCI, MySQL and MariaDB.
//
// The unit tests in `src/ui/bind_prompt` settle which placeholders are found
// and what text or binds come out of an answer. What only a server can settle
// is whether those answers actually run:
//
//   (1) that a prompted value reaches Oracle as a real bind of the declared
//       type, through both drivers — including DATE and TIMESTAMP, where the
//       thin driver parses the text itself and OCI hands it to the client
//       library;
//   (2) that a Number answer works where a string literal would be a syntax
//       error — Oracle `FETCH FIRST :n ROWS ONLY`, MySQL `LIMIT :n`;
//   (3) that the MySQL family's literal substitution produces SQL the server
//       accepts and that quoting survives the round trip;
//   (4) that `VARIABLE` declarations are left alone: a declared bind is never
//       asked about, keeps its value, and still resolves — including in a
//       statement that mixes a declared bind with an undeclared one;
//   (5) that a prompted value is asked about again on the next run instead of
//       freezing into a declaration.
//
// Everything runs through the production path: `SqlEditorWidget::execute_sql_text`
// (what Ctrl+Enter calls), and the modal that opens is the production one. Only
// the pointer is replaced — a timeout fills the rows and clicks a button from
// inside the modal's own event loop.
//
// Usage: verify_bind_prompt_live <thin|oci|mysql|mariadb|all>
// Env: see docs/oracle.md, docs/mysql.md, docs/mariadb.md.
//
// Run one Oracle container at a time.

use fltk::{
    app,
    button::{Button, CheckButton},
    group::Group,
    input::{Input, IntInput},
    menu::Choice,
    prelude::*,
    window::Window,
};
use space_query::db::{ConnectionInfo, DatabaseConnection, DatabaseType, OracleDriverMode};
use space_query::ui::bind_prompt::BindParamType;
use space_query::ui::sql_editor::{QueryProgress, SqlEditorWidget};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const TABLE: &str = "OQT_BIND_EMP";
/// Oracle only: a procedure with an OUT scalar and an OUT ref cursor.
const PROC: &str = "OQT_BIND_P";
/// A procedure with one IN and one OUT scalar, on every backend.
const PROC2: &str = "OQT_BIND_P2";
/// A function of one IN argument, on every backend.
const FUNC: &str = "OQT_BIND_F";
/// One column per data type the app can meet.
const TYPES_TABLE: &str = "OQT_BIND_TYPES";

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

    fn setup_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!(
                    "CREATE TABLE {TABLE} (EMPNO NUMBER PRIMARY KEY, DEPTNO NUMBER, \
                     ENAME VARCHAR2(30), HIREDATE DATE, NOTE VARCHAR2(100))"
                ),
                format!(
                    "INSERT INTO {TABLE} VALUES (7369, 20, 'SMITH', \
                     TO_DATE('1980-12-17','YYYY-MM-DD'), NULL)"
                ),
                format!(
                    "INSERT INTO {TABLE} VALUES (7499, 30, 'ALLEN', \
                     TO_DATE('1981-02-20','YYYY-MM-DD'), 'o''hare')"
                ),
                format!(
                    "INSERT INTO {TABLE} VALUES (7521, 30, 'WARD', \
                     TO_DATE('1981-02-22','YYYY-MM-DD'), NULL)"
                ),
                format!(
                    "CREATE OR REPLACE PROCEDURE {PROC} \
                     (p_dept IN NUMBER, p_count OUT NUMBER, p_rows OUT SYS_REFCURSOR) AS \
                     BEGIN \
                       SELECT COUNT(*) INTO p_count FROM {TABLE} WHERE DEPTNO = p_dept; \
                       OPEN p_rows FOR SELECT ENAME FROM {TABLE} \
                         WHERE DEPTNO = p_dept ORDER BY EMPNO; \
                     END;"
                ),
                format!(
                    "CREATE OR REPLACE PROCEDURE {PROC2} \
                     (p_dept IN NUMBER, p_count OUT NUMBER) AS \
                     BEGIN \
                       SELECT COUNT(*) INTO p_count FROM {TABLE} WHERE DEPTNO = p_dept; \
                     END;"
                ),
                format!(
                    "CREATE OR REPLACE FUNCTION {FUNC} (p_dept IN NUMBER) RETURN NUMBER AS \
                       v_count NUMBER; \
                     BEGIN \
                       SELECT COUNT(*) INTO v_count FROM {TABLE} WHERE DEPTNO = p_dept; \
                       RETURN v_count; \
                     END;"
                ),
            ]
        } else {
            vec![
                format!(
                    "CREATE TABLE {TABLE} (EMPNO INT PRIMARY KEY, DEPTNO INT, \
                     ENAME VARCHAR(30), HIREDATE DATE, NOTE VARCHAR(100))"
                ),
                format!("INSERT INTO {TABLE} VALUES (7369, 20, 'SMITH', '1980-12-17', NULL)"),
                format!("INSERT INTO {TABLE} VALUES (7499, 30, 'ALLEN', '1981-02-20', 'o''hare')"),
                format!("INSERT INTO {TABLE} VALUES (7521, 30, 'WARD', '1981-02-22', NULL)"),
                format!(
                    "CREATE PROCEDURE {PROC2} (IN p_dept INT, OUT p_count INT) \
                     BEGIN SELECT COUNT(*) INTO p_count FROM {TABLE} WHERE DEPTNO = p_dept; END"
                ),
                format!(
                    "CREATE FUNCTION {FUNC} (p_dept INT) RETURNS INT READS SQL DATA \
                     BEGIN DECLARE v_count INT; \
                     SELECT COUNT(*) INTO v_count FROM {TABLE} WHERE DEPTNO = p_dept; \
                     RETURN v_count; END"
                ),
            ]
        }
    }

    fn teardown_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!("DROP FUNCTION {FUNC}"),
                format!("DROP PROCEDURE {PROC2}"),
                format!("DROP PROCEDURE {PROC}"),
                format!("DROP TABLE {TABLE}"),
            ]
        } else {
            vec![
                format!("DROP FUNCTION IF EXISTS {FUNC}"),
                format!("DROP PROCEDURE IF EXISTS {PROC2}"),
                format!("DROP TABLE IF EXISTS {TABLE}"),
            ]
        }
    }

    fn lock_timeout_sql(self) -> &'static str {
        if self.is_oracle() {
            "ALTER SESSION SET ddl_lock_timeout = 5"
        } else {
            "SET SESSION lock_wait_timeout = 5"
        }
    }

    /// One column per data type, and the single row that fills them.
    ///
    /// A comparison against a bind is the point, so the expression column says
    /// how to reach a comparable value where the bare column is not one — an
    /// Oracle `CLOB` cannot sit beside `=`, a `RAW` compares as hex.
    fn types_setup_sql(self) -> Vec<String> {
        if self.is_oracle() {
            vec![
                format!(
                    "CREATE TABLE {TYPES_TABLE} (\
                       ID NUMBER PRIMARY KEY, \
                       N_INT NUMBER, \
                       N_DEC NUMBER(9,2), \
                       N_DBL BINARY_DOUBLE, \
                       V VARCHAR2(30 CHAR), \
                       C CHAR(5), \
                       NV NVARCHAR2(30), \
                       D DATE, \
                       TS TIMESTAMP(6), \
                       TSTZ TIMESTAMP(6) WITH TIME ZONE, \
                       CL CLOB, \
                       R RAW(8))"
                ),
                format!(
                    "INSERT INTO {TYPES_TABLE} VALUES (1, 42, 1234.56, 2.5, \
                       'ünïcode', 'AB', N'héllo', \
                       TO_DATE('1980-12-17','YYYY-MM-DD'), \
                       TO_TIMESTAMP('2026-08-08 10:11:12','YYYY-MM-DD HH24:MI:SS'), \
                       TO_TIMESTAMP_TZ('2026-08-08 10:11:12 +00:00', \
                         'YYYY-MM-DD HH24:MI:SS TZH:TZM'), \
                       'clob-value', HEXTORAW('DEADBEEF'))"
                ),
            ]
        } else {
            vec![
                format!(
                    "CREATE TABLE {TYPES_TABLE} (\
                       ID INT PRIMARY KEY, \
                       N_INT INT, \
                       N_BIG BIGINT, \
                       N_DEC DECIMAL(9,2), \
                       N_DBL DOUBLE, \
                       V VARCHAR(30), \
                       C CHAR(5), \
                       T TEXT, \
                       D DATE, \
                       DT DATETIME, \
                       TS TIMESTAMP NULL, \
                       TM TIME, \
                       B BLOB, \
                       J JSON)"
                ),
                format!(
                    "INSERT INTO {TYPES_TABLE} VALUES (1, 42, 9007199254740993, 1234.56, 2.5, \
                       'ünïcode', 'AB', 'text-value', '1980-12-17', \
                       '2026-08-08 10:11:12', '2026-08-08 10:11:12', '10:11:12', \
                       UNHEX('DEADBEEF'), '{{\"a\": \"x\"}}')"
                ),
            ]
        }
    }

    fn types_teardown_sql(self) -> String {
        if self.is_oracle() {
            format!("DROP TABLE {TYPES_TABLE}")
        } else {
            format!("DROP TABLE IF EXISTS {TYPES_TABLE}")
        }
    }

    /// `(label, bare column, the type the prompt should open on)`.
    ///
    /// One entry per column of the types table, so every data type each backend
    /// can hand back is covered — including the ones whose answer is `String`,
    /// where the assertion is that nothing was guessed.
    fn inference_cases(self) -> Vec<(&'static str, &'static str, BindParamType)> {
        if self.is_oracle() {
            vec![
                ("NUMBER", "N_INT", BindParamType::Number),
                ("NUMBER(9,2)", "N_DEC", BindParamType::Number),
                ("BINARY_DOUBLE", "N_DBL", BindParamType::Number),
                ("VARCHAR2", "V", BindParamType::String),
                ("CHAR", "C", BindParamType::String),
                ("NVARCHAR2", "NV", BindParamType::String),
                ("DATE", "D", BindParamType::Date),
                ("TIMESTAMP", "TS", BindParamType::Timestamp),
                ("TIMESTAMP WITH TIME ZONE", "TSTZ", BindParamType::Timestamp),
                ("CLOB", "CL", BindParamType::String),
                ("RAW", "R", BindParamType::String),
            ]
        } else {
            vec![
                ("INT", "N_INT", BindParamType::Number),
                ("BIGINT", "N_BIG", BindParamType::Number),
                ("DECIMAL(9,2)", "N_DEC", BindParamType::Number),
                ("DOUBLE", "N_DBL", BindParamType::Number),
                ("VARCHAR", "V", BindParamType::String),
                ("CHAR", "C", BindParamType::String),
                ("TEXT", "T", BindParamType::String),
                ("DATE", "D", BindParamType::Date),
                ("DATETIME", "DT", BindParamType::Timestamp),
                ("TIMESTAMP", "TS", BindParamType::Timestamp),
                ("TIME", "TM", BindParamType::String),
                ("BLOB", "B", BindParamType::String),
                ("JSON", "J", BindParamType::String),
            ]
        }
    }

    /// `(label, comparable expression, answer type, answer value)` per column.
    fn type_cases(self) -> Vec<(&'static str, &'static str, BindParamType, &'static str)> {
        if self.is_oracle() {
            vec![
                ("NUMBER", "N_INT", BindParamType::Number, "42"),
                ("NUMBER(9,2)", "N_DEC", BindParamType::Number, "1234.56"),
                ("BINARY_DOUBLE", "N_DBL", BindParamType::Number, "2.5"),
                ("VARCHAR2", "V", BindParamType::String, "ünïcode"),
                ("CHAR", "RTRIM(C)", BindParamType::String, "AB"),
                ("NVARCHAR2", "NV", BindParamType::String, "héllo"),
                ("DATE", "D", BindParamType::Date, "1980-12-17"),
                (
                    "TIMESTAMP",
                    "TS",
                    BindParamType::Timestamp,
                    "2026-08-08 10:11:12",
                ),
                (
                    "TIMESTAMP WITH TIME ZONE",
                    "TO_CHAR(TSTZ, 'YYYY-MM-DD HH24:MI:SS')",
                    BindParamType::String,
                    "2026-08-08 10:11:12",
                ),
                (
                    "CLOB",
                    "DBMS_LOB.SUBSTR(CL, 100)",
                    BindParamType::String,
                    "clob-value",
                ),
                ("RAW", "RAWTOHEX(R)", BindParamType::String, "DEADBEEF"),
            ]
        } else {
            vec![
                ("INT", "N_INT", BindParamType::Number, "42"),
                ("BIGINT", "N_BIG", BindParamType::Number, "9007199254740993"),
                ("DECIMAL(9,2)", "N_DEC", BindParamType::Number, "1234.56"),
                ("DOUBLE", "N_DBL", BindParamType::Number, "2.5"),
                ("VARCHAR", "V", BindParamType::String, "ünïcode"),
                ("CHAR", "C", BindParamType::String, "AB"),
                ("TEXT", "T", BindParamType::String, "text-value"),
                ("DATE", "D", BindParamType::Date, "1980-12-17"),
                (
                    "DATETIME",
                    "DT",
                    BindParamType::Timestamp,
                    "2026-08-08 10:11:12",
                ),
                (
                    "TIMESTAMP",
                    "TS",
                    BindParamType::Timestamp,
                    "2026-08-08 10:11:12",
                ),
                ("TIME", "TM", BindParamType::String, "10:11:12"),
                ("BLOB", "HEX(B)", BindParamType::String, "DEADBEEF"),
                (
                    "JSON",
                    "JSON_UNQUOTE(JSON_EXTRACT(J, '$.a'))",
                    BindParamType::String,
                    "x",
                ),
            ]
        }
    }

    /// A row-limiting clause that only accepts a numeric bind.
    fn limit_sql(self, count: &str) -> String {
        if self.is_oracle() {
            format!("SELECT ENAME FROM {TABLE} ORDER BY EMPNO FETCH FIRST :{count} ROWS ONLY")
        } else {
            format!("SELECT ENAME FROM {TABLE} ORDER BY EMPNO LIMIT :{count}")
        }
    }
}

// --- driving the modal -------------------------------------------------------

/// What the timeout should do to each row of the modal when it appears.
#[derive(Clone)]
struct Answer {
    param_type: BindParamType,
    value: String,
    null: bool,
}

impl Answer {
    fn of(param_type: BindParamType, value: &str) -> Self {
        Self {
            param_type,
            value: value.to_string(),
            null: false,
        }
    }

    fn null() -> Self {
        Self {
            param_type: BindParamType::String,
            value: String::new(),
            null: true,
        }
    }

    /// A PL/SQL OUT parameter: the type is the answer, there is no value.
    fn out(param_type: BindParamType) -> Self {
        Self {
            param_type,
            value: String::new(),
            null: false,
        }
    }
}

struct ModalPlan {
    armed: bool,
    answers: Vec<Answer>,
    cancel: bool,
    /// Parameter names the modal listed, in order.
    seen_labels: Vec<String>,
    /// Values the modal already carried when it opened.
    seen_values: Vec<String>,
    /// Types the modal already had selected when it opened, which is what the
    /// inference chose.
    seen_types: Vec<String>,
    appeared: bool,
    attempts: u32,
}

static PLAN: OnceLock<Mutex<ModalPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<ModalPlan> {
    PLAN.get_or_init(|| {
        Mutex::new(ModalPlan {
            armed: false,
            answers: Vec::new(),
            cancel: false,
            seen_labels: Vec::new(),
            seen_values: Vec::new(),
            seen_types: Vec::new(),
            appeared: false,
            attempts: 0,
        })
    })
}

fn arm_modal(answers: Vec<Answer>, cancel: bool) {
    {
        let mut plan = plan().lock().unwrap_or_else(|p| p.into_inner());
        plan.armed = true;
        plan.answers = answers;
        plan.cancel = cancel;
        plan.seen_labels.clear();
        plan.seen_values.clear();
        plan.seen_types.clear();
        plan.appeared = false;
        plan.attempts = 0;
    }
    app::add_timeout3(0.10, |_| drive_modal());
}

fn drive_modal() {
    if !plan().lock().unwrap_or_else(|p| p.into_inner()).armed {
        return;
    }
    let Some(dialog) = window_by_label("Bind Parameters") else {
        let mut plan = plan().lock().unwrap_or_else(|p| p.into_inner());
        plan.attempts += 1;
        // Roughly three seconds. A case that expects no modal ends here, which
        // is exactly the assertion it makes.
        if plan.attempts > 60 {
            plan.armed = false;
            return;
        }
        drop(plan);
        app::add_timeout3(0.05, |_| drive_modal());
        return;
    };
    let Some(group) = dialog.as_group() else {
        return;
    };
    let mut widgets = Vec::new();
    collect_widgets(&group, &mut widgets);

    let labels: Vec<String> = widgets
        .iter()
        .filter(|widget| {
            // The row's name sits in a plain Frame; every other label in the
            // dialog belongs to a button, a checkbox or the hint.
            widget.label().starts_with(':') || widget.label().starts_with('?')
        })
        .map(|widget| widget.label())
        .collect();
    let mut choices: Vec<Choice> = widgets.iter().filter_map(Choice::from_dyn_widget).collect();
    let mut inputs: Vec<Input> = widgets.iter().filter_map(Input::from_dyn_widget).collect();
    let mut checks: Vec<CheckButton> = widgets
        .iter()
        .filter_map(CheckButton::from_dyn_widget)
        .collect();

    let mut plan = plan().lock().unwrap_or_else(|p| p.into_inner());
    plan.appeared = true;
    plan.armed = false;
    plan.seen_labels = labels;
    plan.seen_values = inputs.iter().map(Input::value).collect();
    // Read before the answers below overwrite them: this is the selection the
    // prompt opened with, not the one this harness makes.
    plan.seen_types = choices
        .iter()
        .map(|choice| choice.choice().unwrap_or_default())
        .collect();

    let answers = plan.answers.clone();
    for (index, answer) in answers.iter().enumerate() {
        if let Some(choice) = choices.get_mut(index) {
            let wanted = BindParamType::ALL
                .iter()
                .position(|candidate| *candidate == answer.param_type)
                .unwrap_or_default();
            choice.set_value(wanted as i32);
            // Setting a value by hand does not fire FLTK's callback, and the
            // callbacks are what enable and disable the rest of the row.
            choice.do_callback();
        }
        if let Some(input) = inputs.get_mut(index) {
            input.set_value(&answer.value);
        }
        if let Some(check) = checks.get_mut(index) {
            check.set_value(answer.null);
            check.do_callback();
        }
    }

    let wanted_button = if plan.cancel { "Cancel" } else { "Run" };
    drop(plan);

    for widget in &widgets {
        if let Some(mut button) = Button::from_dyn_widget(widget) {
            if button.label() == wanted_button {
                button.do_callback();
                return;
            }
        }
    }
    let mut dialog = dialog;
    dialog.hide();
}

fn collect_widgets(group: &Group, out: &mut Vec<fltk::widget::Widget>) {
    for child in group.clone().into_iter() {
        if let Some(child_group) = child.as_group() {
            collect_widgets(&child_group, out);
        }
        out.push(child);
    }
}

fn window_by_label(label: &str) -> Option<Window> {
    let mut current = app::first_window().map(|window| unsafe { Window::from_widget(window) });
    while let Some(window) = current {
        current = app::next_window(&window).map(|next| unsafe { Window::from_widget(next) });
        if window.shown() && window.label() == label {
            return Some(window);
        }
    }
    None
}

// --- harness -----------------------------------------------------------------

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

/// Every row the run produced.
///
/// A streamed SELECT delivers its rows in `Rows` events and finishes with a
/// summary that carries none, so both shapes have to be read or a passing query
/// looks empty.
fn collected_rows(events: &[QueryProgress]) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for event in events {
        match progress_inner(event) {
            QueryProgress::Rows { rows: batch, .. } => rows.extend(batch.iter().cloned()),
            QueryProgress::StatementFinished { result, .. } => {
                rows.extend(result.rows.iter().cloned())
            }
            _ => {}
        }
    }
    rows
}

/// The messages the run's statements reported, joined.
///
/// An OUT assignment is reported here rather than as a row: a PL/SQL call
/// appends the assigned binds to its success message.
fn finished_messages(events: &[QueryProgress]) -> String {
    events
        .iter()
        .filter_map(|event| match progress_inner(event) {
            QueryProgress::StatementFinished { result, .. } => Some(result.message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
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

/// What one prompted run produced.
struct RunOutcome {
    rows: Vec<Vec<String>>,
    /// Parameter names the modal listed, empty when no modal opened.
    prompted: Vec<String>,
    /// Values the modal already carried when it opened.
    prefilled: Vec<String>,
    /// Types the modal already had selected when it opened.
    preselected: Vec<String>,
    modal_appeared: bool,
    /// The messages the run's statements reported, joined.
    message: String,
}

struct Harness {
    editor: SqlEditorWidget,
    events: Arc<Mutex<Vec<QueryProgress>>>,
    done: Arc<AtomicBool>,
}

impl Harness {
    /// Run `sql` with no placeholders in it, failing on any server error.
    fn run(&mut self, sql: &str) -> Result<Vec<QueryProgress>, String> {
        let outcome = self.run_raw(sql, Vec::new(), false, false)?;
        if outcome.modal_appeared {
            return Err(format!("unexpected bind prompt for: {sql}"));
        }
        Ok(self.events())
    }

    /// Run `sql`, answering the prompt it is expected to raise.
    fn prompt(&mut self, sql: &str, answers: Vec<Answer>) -> Result<RunOutcome, String> {
        self.run_raw(sql, answers, false, false)
    }

    fn prompt_script(&mut self, sql: &str, answers: Vec<Answer>) -> Result<RunOutcome, String> {
        self.run_raw(sql, answers, false, true)
    }

    /// Run `sql` and cancel the prompt instead of answering it.
    fn cancel(&mut self, sql: &str) -> Result<RunOutcome, String> {
        self.run_raw(sql, Vec::new(), true, false)
    }

    fn run_raw(
        &mut self,
        sql: &str,
        answers: Vec<Answer>,
        cancel: bool,
        script: bool,
    ) -> Result<RunOutcome, String> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.done.store(false, Ordering::SeqCst);
        arm_modal(answers, cancel);

        // The modal opens inside this call and runs its own event loop, so the
        // timeout armed above is what lets this return.
        if script {
            self.editor.execute_script_for_harness(sql);
        } else {
            self.editor.execute_sql_text(sql);
        }

        let (appeared, cancelled) = {
            let plan = plan().lock().unwrap_or_else(|p| p.into_inner());
            (plan.appeared, plan.cancel)
        };
        if !(appeared && cancelled) {
            let done = Arc::clone(&self.done);
            pump_until("statement to finish", 120, || done.load(Ordering::SeqCst))?;
        } else {
            // Nothing was started; give any stray callback a moment to land.
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline {
                if !app::wait() {
                    app::check();
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }

        let plan = plan().lock().unwrap_or_else(|p| p.into_inner());
        let events = self.events();
        if let Some(error) = first_error(&events) {
            return Err(error);
        }
        Ok(RunOutcome {
            message: finished_messages(&events),
            rows: collected_rows(&events),
            prompted: plan.seen_labels.clone(),
            prefilled: plan.seen_values.clone(),
            preselected: plan.seen_types.clone(),
            modal_appeared: plan.appeared,
        })
    }

    fn events(&self) -> Vec<QueryProgress> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Prefix of the hidden snapshot the MySQL family appends to an editable
/// result (`RESULT_EDIT_SNAPSHOT_PREFIX` in `src/db/result_edit.rs`).
const EDIT_SNAPSHOT_PREFIX: &str = "\x1eSQ_EDIT_V1:";

/// The cells of `row` a user would see.
///
/// An editable result carries bookkeeping the grid hides: Oracle prepends a
/// ROWID, the MySQL family appends an edit snapshot. Every query here selects
/// one expression, so the wanted value is the last cell that is not one of
/// those.
fn visible_cells(row: &[String]) -> Vec<&String> {
    row.iter()
        .filter(|cell| !cell.starts_with(EDIT_SNAPSHOT_PREFIX))
        .collect()
}

/// The value of the first row's last visible column.
fn one_value(rows: &[Vec<String>]) -> String {
    rows.first()
        .and_then(|row| visible_cells(row).last().map(|cell| (*cell).clone()))
        .unwrap_or_default()
}

/// Assert that a call reported the OUT assignment it was supposed to make.
fn expect_out(outcome: &RunOutcome, needle: &str, what: &str) -> Result<(), String> {
    if !outcome.message.contains(needle) {
        return Err(format!(
            "{what}: expected {needle:?} in the result message, got {:?}",
            outcome.message
        ));
    }
    Ok(())
}

fn expect_single(outcome: &RunOutcome, expected: &str, what: &str) -> Result<(), String> {
    if outcome.rows.len() != 1 || one_value(&outcome.rows) != expected {
        return Err(format!(
            "{what}: expected exactly [{expected}], got {:?}",
            outcome.rows
        ));
    }
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
    let _ = h.run(&target.types_teardown_sql());
    for sql in target.teardown_sql() {
        let _ = h.run(&sql);
    }
    for sql in target.setup_sql() {
        h.run(&sql).map_err(|e| format!("setup ({sql}): {e}"))?;
    }
    for sql in target.types_setup_sql() {
        h.run(&sql)
            .map_err(|e| format!("types setup ({sql}): {e}"))?;
    }
    let _ = h.run("COMMIT");

    // (1) A named bind carrying a number.
    let outcome = h.prompt(
        &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id"),
        vec![Answer::of(BindParamType::Number, "7369")],
    )?;
    if outcome.prompted != vec![":ID".to_string()] {
        return Err(format!(
            "expected one :ID prompt, got {:?}",
            outcome.prompted
        ));
    }
    expect_single(&outcome, "SMITH", "number bind")?;
    println!("PASS: :id = 7369 (Number) selected SMITH");

    // (2) A named bind carrying text, including a quote the substitution must
    //     escape on the MySQL family.
    let outcome = h.prompt(
        &format!("SELECT ENAME FROM {TABLE} WHERE NOTE = :note"),
        vec![Answer::of(BindParamType::String, "o'hare")],
    )?;
    expect_single(&outcome, "ALLEN", "string bind with a quote")?;
    println!("PASS: :note = o'hare (String) selected ALLEN");

    // (3) One name used twice is asked about once and applied to both.
    let outcome = h.prompt(
        &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id OR EMPNO = :id"),
        vec![Answer::of(BindParamType::Number, "7369")],
    )?;
    if outcome.prompted.len() != 1 {
        return Err(format!(
            "a repeated name was asked about {} times",
            outcome.prompted.len()
        ));
    }
    expect_single(&outcome, "SMITH", "repeated name")?;
    println!("PASS: a name used twice is asked about once");

    // (4) A row-limiting clause, where a string literal would not parse.
    let outcome = h.prompt(
        &target.limit_sql("n"),
        vec![Answer::of(BindParamType::Number, "2")],
    )?;
    if outcome.rows.len() != 2 {
        return Err(format!(
            "row limit bind returned {} rows, expected 2",
            outcome.rows.len()
        ));
    }
    println!("PASS: a Number answer works in the row-limiting clause");

    // (5) A DATE value, given as ISO text.
    let outcome = h.prompt(
        &format!("SELECT ENAME FROM {TABLE} WHERE HIREDATE = :d"),
        vec![Answer::of(BindParamType::Date, "1980-12-17")],
    )?;
    expect_single(&outcome, "SMITH", "date bind")?;
    println!("PASS: :d = 1980-12-17 (Date) selected SMITH");

    // (6) NULL.
    let outcome = h.prompt(
        &format!("SELECT COUNT(*) FROM {TABLE} WHERE NOTE IS NULL AND :note IS NULL"),
        vec![Answer::null()],
    )?;
    if one_value(&outcome.rows) != "2" {
        return Err(format!("NULL bind counted {:?}, expected 2", outcome.rows));
    }
    println!("PASS: a NULL answer is bound as SQL NULL");

    // (7) Two positional placeholders, matched in order.
    let outcome = h.prompt(
        &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = ? AND DEPTNO = ?"),
        vec![
            Answer::of(BindParamType::Number, "7369"),
            Answer::of(BindParamType::Number, "20"),
        ],
    )?;
    if outcome.prompted != vec!["? 1".to_string(), "? 2".to_string()] {
        return Err(format!(
            "expected two positional prompts, got {:?}",
            outcome.prompted
        ));
    }
    expect_single(&outcome, "SMITH", "positional placeholders")?;
    println!("PASS: ? and ? are matched in order");

    // (8) A colon inside a literal is not a placeholder, so nothing is asked.
    let quoted = if target.is_oracle() {
        format!("SELECT TO_CHAR(HIREDATE, 'HH24:MI:SS') FROM {TABLE} WHERE EMPNO = 7369")
    } else {
        format!("SELECT DATE_FORMAT(HIREDATE, '%H:%i:%s') FROM {TABLE} WHERE EMPNO = 7369")
    };
    let outcome = h.prompt(&quoted, Vec::new())?;
    if outcome.modal_appeared {
        return Err(format!(
            "a colon inside a literal raised a prompt: {:?}",
            outcome.prompted
        ));
    }
    if one_value(&outcome.rows) != "00:00:00" {
        return Err(format!(
            "the format model did not survive: {:?}",
            outcome.rows
        ));
    }
    println!("PASS: a colon inside a literal asks nothing and runs unchanged");

    // (9) A prompted value is asked about again next time, prefilled.
    let sql = format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id");
    let outcome = h.prompt(&sql, vec![Answer::of(BindParamType::Number, "7499")])?;
    expect_single(&outcome, "ALLEN", "second answer for :id")?;
    let outcome = h.prompt(&sql, vec![Answer::of(BindParamType::Number, "7521")])?;
    if !outcome.modal_appeared {
        return Err("a prompted bind froze into a declaration".to_string());
    }
    if outcome.prefilled.first().map(String::as_str) != Some("7499") {
        return Err(format!(
            "the prompt did not prefill the previous answer: {:?}",
            outcome.prefilled
        ));
    }
    expect_single(&outcome, "WARD", "third answer for :id")?;
    println!("PASS: a prompted bind is asked again, prefilled with the last answer");

    // (10) Cancel runs nothing at all.
    let outcome = h.cancel(&format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id"))?;
    if !outcome.modal_appeared {
        return Err("the cancel case never saw a prompt".to_string());
    }
    if !outcome.rows.is_empty() {
        return Err(format!(
            "cancel still ran the statement: {:?}",
            outcome.rows
        ));
    }
    println!("PASS: cancelling the prompt runs nothing");

    if target.is_oracle() {
        // (11) A `VARIABLE` declaration is never asked about.
        let outcome = h.prompt_script(
            &format!(
                "VARIABLE id NUMBER\n\
                 EXEC :id := 7369\n\
                 SELECT ENAME FROM {TABLE} WHERE EMPNO = :id;"
            ),
            Vec::new(),
        )?;
        if outcome.modal_appeared {
            return Err(format!(
                "a VARIABLE declaration was asked about: {:?}",
                outcome.prompted
            ));
        }
        expect_single(&outcome, "SMITH", "declared bind")?;
        println!("PASS: a VARIABLE declaration runs without a prompt");

        // (12) The declaration above is still in the session. A statement that
        //      mixes it with an undeclared name must ask about the second one
        //      only, and the declared value must still resolve.
        let outcome = h.prompt(
            &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id AND DEPTNO = :dept"),
            vec![Answer::of(BindParamType::Number, "20")],
        )?;
        if outcome.prompted != vec![":DEPT".to_string()] {
            return Err(format!(
                "the mixed statement asked about {:?}, expected only :DEPT",
                outcome.prompted
            ));
        }
        expect_single(&outcome, "SMITH", "mixed declared and prompted binds")?;
        println!("PASS: a declared bind keeps its value while the other is prompted");

        // (13) …and the declared bind is still not asked about on a later run,
        //      even though a prompted bind now sits beside it in the session.
        let outcome = h.prompt(
            &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id AND DEPTNO = :dept"),
            vec![Answer::of(BindParamType::Number, "20")],
        )?;
        if outcome.prompted != vec![":DEPT".to_string()] {
            return Err(format!(
                "the declared bind was asked about on the second run: {:?}",
                outcome.prompted
            ));
        }
        expect_single(&outcome, "SMITH", "mixed binds, second run")?;
        println!("PASS: the declaration still stands after a prompt beside it");

        // (14) A declared bind mixed with a positional placeholder: the
        //      generated name must not collide with anything declared.
        let outcome = h.prompt(
            &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id AND DEPTNO = ?"),
            vec![Answer::of(BindParamType::Number, "20")],
        )?;
        if outcome.prompted != vec!["? 1".to_string()] {
            return Err(format!(
                "the declared bind was asked about beside a ?: {:?}",
                outcome.prompted
            ));
        }
        expect_single(&outcome, "SMITH", "declared bind beside a positional")?;
        println!("PASS: a declared bind mixes with a positional placeholder");

        // (15) A TIMESTAMP value, given as ISO text.
        let outcome = h.prompt(
            "SELECT TO_CHAR(CAST(:t AS TIMESTAMP), 'YYYY-MM-DD HH24:MI:SS') FROM DUAL",
            vec![Answer::of(BindParamType::Timestamp, "2026-08-08 10:11:12")],
        )?;
        if one_value(&outcome.rows) != "2026-08-08 10:11:12" {
            return Err(format!("timestamp bind came back as {:?}", outcome.rows));
        }
        println!("PASS: a Timestamp answer round-trips");

        // (16) An undeclared OUT ref cursor, answered as one. Its rows are what
        //      the call produces, so a wrong bind type would show up as an
        //      error rather than as data.
        let outcome = h.prompt(
            &format!("BEGIN {PROC}(:dept, :cnt, :rc); END;"),
            vec![
                Answer::of(BindParamType::Number, "30"),
                Answer::out(BindParamType::Number),
                Answer::out(BindParamType::RefCursor),
            ],
        )?;
        if outcome.prompted != vec![":DEPT".to_string(), ":CNT".to_string(), ":RC".to_string()] {
            return Err(format!("the OUT call asked about {:?}", outcome.prompted));
        }
        let names: Vec<String> = outcome
            .rows
            .iter()
            .filter_map(|row| visible_cells(row).last().map(|cell| (*cell).clone()))
            .collect();
        if !names.contains(&"ALLEN".to_string()) || !names.contains(&"WARD".to_string()) {
            return Err(format!("the OUT ref cursor produced {names:?}"));
        }
        println!("PASS: an undeclared OUT ref cursor is answered as a Ref Cursor");

        // (17) The OUT scalar came back into the session, and the next run still
        //      asks about it — a prompted bind is never a declaration.
        let outcome = h.prompt(
            &format!("BEGIN {PROC}(:dept, :cnt, :rc); END;"),
            vec![
                Answer::of(BindParamType::Number, "20"),
                Answer::out(BindParamType::Number),
                Answer::out(BindParamType::RefCursor),
            ],
        )?;
        if outcome.prompted.len() != 3 {
            return Err(format!(
                "the OUT call asked about {:?} on its second run",
                outcome.prompted
            ));
        }
        let names: Vec<String> = outcome
            .rows
            .iter()
            .filter_map(|row| visible_cells(row).last().map(|cell| (*cell).clone()))
            .collect();
        if !names.contains(&"SMITH".to_string()) {
            return Err(format!("the second OUT call produced {names:?}"));
        }
        println!("PASS: an OUT call keeps being prompted and re-runs cleanly");

        // (18) Every spelling of a procedure call. `EXEC` matters most: it is
        //      rewritten into a PL/SQL block deep in the execution worker, long
        //      after the prompt scanned the text the user wrote.
        for (label, sql) in [
            ("BEGIN … END;", format!("BEGIN {PROC2}(:dept, :cnt); END;")),
            ("EXEC", format!("EXEC {PROC2}(:dept, :cnt)")),
            ("EXECUTE", format!("EXECUTE {PROC2}(:dept, :cnt)")),
            ("CALL", format!("CALL {PROC2}(:dept, :cnt)")),
            (
                "DECLARE … BEGIN … END;",
                format!("DECLARE v NUMBER; BEGIN {PROC2}(:dept, v); :cnt := v; END;"),
            ),
        ] {
            let outcome = h.prompt(
                &sql,
                vec![
                    Answer::of(BindParamType::Number, "30"),
                    Answer::out(BindParamType::Number),
                ],
            )?;
            if outcome.prompted != vec![":DEPT".to_string(), ":CNT".to_string()] {
                return Err(format!(
                    "{label} asked about {:?}, expected :DEPT and :CNT",
                    outcome.prompted
                ));
            }
            expect_out(&outcome, ":CNT = 2", label)?;
            println!("PASS: procedure call form {label} binds and assigns");
        }

        // (19) Functions, in each shape that can carry a bind.
        let outcome = h.prompt(
            &format!("SELECT {FUNC}(:dept) FROM DUAL"),
            vec![Answer::of(BindParamType::Number, "30")],
        )?;
        expect_single(&outcome, "2", "function in a SELECT")?;
        println!("PASS: function call form SELECT f(:x) FROM DUAL returns 2");

        for (label, sql) in [
            (
                "BEGIN :r := f(:x); END;",
                format!("BEGIN :r := {FUNC}(:dept); END;"),
            ),
            ("EXEC :r := f(:x)", format!("EXEC :r := {FUNC}(:dept)")),
        ] {
            let outcome = h.prompt(
                &sql,
                vec![
                    Answer::out(BindParamType::Number),
                    Answer::of(BindParamType::Number, "30"),
                ],
            )?;
            if outcome.prompted != vec![":R".to_string(), ":DEPT".to_string()] {
                return Err(format!(
                    "{label} asked about {:?}, expected :R and :DEPT",
                    outcome.prompted
                ));
            }
            expect_out(&outcome, ":R = 2", label)?;
            println!("PASS: function call form {label} binds its return value");
        }
    } else {
        // (11) The MySQL family has no declarations, so the mixed case there is
        //      a named placeholder beside a positional one.
        let outcome = h.prompt(
            &format!("SELECT ENAME FROM {TABLE} WHERE EMPNO = :id AND DEPTNO = ?"),
            vec![
                Answer::of(BindParamType::Number, "7369"),
                Answer::of(BindParamType::Number, "20"),
            ],
        )?;
        if outcome.prompted != vec![":ID".to_string(), "? 1".to_string()] {
            return Err(format!(
                "the mixed statement asked about {:?}",
                outcome.prompted
            ));
        }
        expect_single(&outcome, "SMITH", "named and positional together")?;
        println!("PASS: a named and a positional placeholder mix");

        // (12) A DATETIME value.
        let outcome = h.prompt(
            "SELECT DATE_FORMAT(CAST(:t AS DATETIME), '%Y-%m-%d %H:%i:%s')",
            vec![Answer::of(BindParamType::Timestamp, "2026-08-08 10:11:12")],
        )?;
        if one_value(&outcome.rows) != "2026-08-08 10:11:12" {
            return Err(format!("timestamp bind came back as {:?}", outcome.rows));
        }
        println!("PASS: a Timestamp answer round-trips");

        // (13) A procedure call. A MySQL OUT argument has to be a user
        //      variable, and `@cnt` is not a placeholder — so it passes through
        //      untouched while the IN value is substituted.
        let outcome = h.prompt(
            &format!("CALL {PROC2}(:dept, @cnt)"),
            vec![Answer::of(BindParamType::Number, "30")],
        )?;
        if outcome.prompted != vec![":DEPT".to_string()] {
            return Err(format!(
                "the CALL asked about {:?}, expected only :DEPT",
                outcome.prompted
            ));
        }
        let events = h.run("SELECT @cnt")?;
        if one_value(&collected_rows(&events)) != "2" {
            return Err(format!(
                "the procedure's OUT variable came back as {:?}",
                collected_rows(&events)
            ));
        }
        println!("PASS: procedure call form CALL p(:x, @out) substitutes only the value");

        // (14) A function.
        let outcome = h.prompt(
            &format!("SELECT {FUNC}(:dept)"),
            vec![Answer::of(BindParamType::Number, "30")],
        )?;
        expect_single(&outcome, "2", "function in a SELECT")?;
        println!("PASS: function call form SELECT f(:x) returns 2");
    }

    // (20) One prompted bind per data type the app can meet. A value that does
    //      not survive the round trip — a lost non-ASCII character, a number
    //      quoted into a string, a date read in the wrong format — fails to
    //      match its row, so the empty result is the assertion.
    for (label, expression, param_type, value) in target.type_cases() {
        let sql = format!("SELECT ID FROM {TYPES_TABLE} WHERE {expression} = :v");
        let outcome = h.prompt(&sql, vec![Answer::of(param_type, value)])?;
        if outcome.prompted != vec![":V".to_string()] {
            return Err(format!("{label} asked about {:?}", outcome.prompted));
        }
        expect_single(&outcome, "1", &format!("{label} bind"))?;
        println!(
            "PASS: a {} answer matches a {label} column",
            param_type.label()
        );
    }

    // (21) The type the prompt opens on, for every data type the backend has.
    //      Cancelled rather than answered: nothing needs to run for the modal
    //      to have chosen, and cancelling keeps a column that cannot sit beside
    //      `=` (a CLOB, a BLOB) out of the server's way.
    for (label, column, expected) in target.inference_cases() {
        // A name this run has not answered before: a remembered answer is the
        // user's own decision and outranks the catalog, which is the right
        // behaviour and the wrong thing to measure here.
        let sql = format!("SELECT ID FROM {TYPES_TABLE} WHERE {column} = :chk_{column}");
        let outcome = h.cancel(&sql)?;
        if outcome.preselected != vec![expected.label().to_string()] {
            return Err(format!(
                "a {label} column opened the prompt on {:?}, expected {:?}",
                outcome.preselected,
                expected.label()
            ));
        }
        println!(
            "PASS: a {label} column opens the prompt on {}",
            expected.label()
        );
    }

    // (22) The same lookup through an alias, and through an INSERT's column
    //      list, which pairs values with columns by position rather than by an
    //      operator.
    let alias_sql = format!(
        "SELECT t.ID FROM {TYPES_TABLE} t WHERE t.D = :chk_alias_d AND t.N_INT = :chk_alias_n"
    );
    let outcome = h.cancel(&alias_sql)?;
    if outcome.preselected
        != vec![
            BindParamType::Date.label().to_string(),
            BindParamType::Number.label().to_string(),
        ]
    {
        return Err(format!(
            "an aliased comparison opened the prompt on {:?}",
            outcome.preselected
        ));
    }
    println!("PASS: an aliased column still names its own type");

    let insert_sql = format!(
        "INSERT INTO {TYPES_TABLE} (ID, V, D) VALUES (:chk_ins_id, :chk_ins_v, :chk_ins_d)"
    );
    let outcome = h.cancel(&insert_sql)?;
    if outcome.preselected
        != vec![
            BindParamType::Number.label().to_string(),
            BindParamType::String.label().to_string(),
            BindParamType::Date.label().to_string(),
        ]
    {
        return Err(format!(
            "an INSERT opened the prompt on {:?}",
            outcome.preselected
        ));
    }
    println!("PASS: an INSERT pairs each value with its own column's type");

    // (23) A row count has no column to look up and is settled by the syntax
    //      alone — where a String answer would be a parse error.
    let count_sql = if target.is_oracle() {
        format!("SELECT ID FROM {TYPES_TABLE} FETCH FIRST :chk_rows ROWS ONLY")
    } else {
        format!("SELECT ID FROM {TYPES_TABLE} LIMIT :chk_rows")
    };
    let outcome = h.cancel(&count_sql)?;
    if outcome.preselected != vec![BindParamType::Number.label().to_string()] {
        return Err(format!(
            "a row count opened the prompt on {:?}",
            outcome.preselected
        ));
    }
    println!("PASS: a row count opens the prompt on Number without a column to consult");

    let _ = h.run(&target.types_teardown_sql());
    for sql in target.teardown_sql() {
        let _ = h.run(&sql);
    }
    Ok(())
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
