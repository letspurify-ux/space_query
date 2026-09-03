#![allow(clippy::cargo, clippy::pedantic)]

// End-to-end verification of result export through the running application.
//
// `verify_result_export` proves the serializers emit each format correctly, but
// it calls them directly. Everything between a user pressing Ctrl+E and that
// call was untested: the menu dispatch, the modal's widget wiring, the format
// and scope the modal reports back, the snapshot the grid takes, and the
// delivery that follows.
//
// This drives the real `MainWindow` with its real callbacks installed. Nothing
// is stubbed: the export is started through the application's own menu bar, the
// modal that opens is the production one, and the only thing replaced is the
// pointer — a timeout sets the modal's controls and clicks its Export button
// the way a user would, from inside the modal's own event loop.
//
// The clipboard destination is checked all the way through, because it ends in
// the OS clipboard and can be read back with `pbpaste`. The file destination
// stops at the macOS save panel, which no in-process code can drive; everything
// before it is shared with the clipboard path, and the write itself is one
// `fs::write`.
//
// `SQL Inserts` is not offered here: the modal hides it without a connection,
// which is itself asserted below. It is covered by `verify_grid_sql_export` and
// `verify_grid_sql_export_live`.
//
// Usage: cargo run --bin verify_result_export_ui

use fltk::{
    app,
    button::{Button, RadioRoundButton},
    enums::Event,
    group::Group,
    menu::{Choice, MenuBar},
    prelude::*,
    window::Window,
};
use space_query::db::{ColumnInfo, DatabaseType, QueryResult, SqlValueKind};
use space_query::ui::grid_sql_export::{build_sql_inserts, SqlWriteDialect};
use space_query::ui::result_export::{
    render, ExportFormat, ExportGrid, ExportPayload, ExportScope,
};
use space_query::ui::result_table::{ExportAbandonReason, LazyFetchCallback};
use space_query::ui::{MainWindow, ResultTableWidget};
use space_query::utils::{arithmetic::safe_div, AppConfig};
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

const NULL_TEXT: &str = "NULL";
const MENU_PATH: &str = "&Tools/&Export Results";

/// What the timeout should do to the modal when it appears.
struct ModalPlan {
    format: ExportFormat,
    scope: ExportScope,
    /// Click Cancel instead of Export.
    cancel: bool,
    /// Every format label the modal offered, filled in by the timeout.
    offered: Vec<String>,
    /// Set once the timeout has found and driven the modal.
    driven: bool,
    /// Set when the export refused with an alert instead of opening the modal.
    refused: bool,
    /// Retries spent waiting for the modal, so a miss ends the run.
    attempts: u32,
}

static PLAN: OnceLock<Mutex<ModalPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<ModalPlan> {
    PLAN.get_or_init(|| {
        Mutex::new(ModalPlan {
            format: ExportFormat::Csv,
            scope: ExportScope::All,
            cancel: false,
            offered: Vec::new(),
            driven: false,
            refused: false,
            attempts: 0,
        })
    })
}

fn column(name: &str, kind: SqlValueKind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: format!("{kind:?}"),
        kind,
    }
}

/// A grid with one value per escaping rule, so a format that loses its escaping
/// somewhere between the menu and the clipboard shows up here.
fn sample_result() -> QueryResult {
    let rows: Vec<Vec<String>> = sample_rows();
    QueryResult {
        sql: "SELECT EMPNO, ENAME, HIREDATE, NOTE FROM SCOTT.EMP".into(),
        row_count: rows.len(),
        execution_time: std::time::Duration::from_millis(12),
        message: format!("{} rows selected", rows.len()),
        is_select: true,
        success: true,
        columns: vec![
            column("EMPNO", SqlValueKind::Number),
            column("ENAME", SqlValueKind::String),
            column("HIREDATE", SqlValueKind::Temporal),
            column("NOTE", SqlValueKind::String),
        ],
        rows,
    }
}

fn sample_rows() -> Vec<Vec<String>> {
    vec![
        vec![
            "7369".into(),
            "comma, and \"quotes\"".into(),
            "1980-12-17 00:00:00".into(),
            "pipe | and <tag>".into(),
        ],
        vec![
            "7499".into(),
            "ALLEN".into(),
            "1981-02-20 00:00:00".into(),
            NULL_TEXT.into(),
        ],
        vec![
            "7521".into(),
            "한국어".into(),
            "1981-02-22 00:00:00".into(),
            "line1\nline2".into(),
        ],
    ]
}

/// The grid the widget should hand the serializers, rebuilt from the fixture.
fn expected_grid(scope: ExportScope) -> ExportGrid {
    let result = sample_result();
    let rows = match scope {
        ExportScope::All => result.rows.clone(),
        // The driven selection below covers the first two rows and the first
        // three columns.
        ExportScope::Selection => result.rows[..2]
            .iter()
            .map(|row| row[..3].to_vec())
            .collect(),
    };
    let keep = match scope {
        ExportScope::All => result.columns.len(),
        ExportScope::Selection => 3,
    };
    ExportGrid {
        columns: result.columns[..keep]
            .iter()
            .map(|column| column.name.clone())
            .collect(),
        column_kinds: result.columns[..keep]
            .iter()
            .map(|column| column.kind)
            .collect(),
        // The grid's own export snapshot reads a cell whose text IS the NULL
        // display text as the absence of a value; the expectation states the
        // same thing.
        rows: rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| (value != NULL_TEXT).then_some(value))
                    .collect()
            })
            .collect(),
        null_text: NULL_TEXT.to_string(),
    }
}

/// Let FLTK settle for roughly `milliseconds`, the way the capture tour does.
fn pump(milliseconds: u64) {
    for _ in 0..safe_div(milliseconds, 20).max(1) {
        app::check();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn say(message: &str) {
    println!("{message}");
    let _ = std::io::stdout().flush();
}

fn main() {
    let mut failures: Vec<String> = Vec::new();

    let _app = app::App::default();
    let mut main_window = MainWindow::new_with_config(AppConfig::default());
    main_window.setup_callbacks();
    main_window.show();
    pump(600);

    // The result attaches to the active editor tab's workspace, so that tab has
    // to exist and be settled before a result can land in it.
    let _ = main_window.capture_tour_set_sql("SELECT * FROM SCOTT.EMP;", Some(0));
    pump(300);

    if let Err(err) = main_window.capture_tour_show_result("Result", sample_result(), false, None) {
        eprintln!("could not show the fixture result: {err}");
        std::process::exit(1);
    }
    pump(800);
    if !main_window.capture_tour_result_has_data() {
        eprintln!("the fixture result never reached the grid");
        std::process::exit(1);
    }
    say("PASS: the fixture result is on screen and exportable");

    // Every format the modal offers without a connection, plus both scopes on
    // one of them, plus the cancel path.
    let mut cases: Vec<(ExportFormat, ExportScope)> = ExportFormat::ALL
        .into_iter()
        .filter(|format| *format != ExportFormat::SqlInserts)
        .map(|format| (format, ExportScope::All))
        .collect();
    cases.push((ExportFormat::Json, ExportScope::Selection));

    for (format, scope) in cases {
        match run_export(&mut main_window, format, scope, false) {
            Ok(clipboard) => {
                let expected = render(format, &expected_grid(scope));
                if clipboard == expected {
                    say(&format!(
                        "PASS: {} / {scope:?} reached the clipboard byte for byte",
                        format.label()
                    ));
                } else {
                    failures.push(format!(
                        "{} / {scope:?}: clipboard held {clipboard:?}, expected {expected:?}",
                        format.label()
                    ));
                }
            }
            Err(err) => failures.push(format!("{} / {scope:?}: {err}", format.label())),
        }
    }

    // The modal must hide SQL Inserts with no connection, since its literals
    // depend on the dialect.
    let offered = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .offered
        .clone();
    if offered.iter().any(|label| label == "SQL Inserts") {
        failures.push("the modal offered SQL Inserts without a connection".to_string());
    } else {
        say("PASS: SQL Inserts is withheld while no connection can name the dialect");
    }
    say(&format!("      formats offered: {}", offered.join(", ")));

    // Cancel must leave the clipboard alone.
    set_clipboard("sentinel");
    match run_export(&mut main_window, ExportFormat::Json, ExportScope::All, true) {
        Ok(clipboard) if clipboard == "sentinel" => {
            say("PASS: cancelling the modal exports nothing");
        }
        Ok(clipboard) => {
            failures.push(format!(
                "cancel still wrote to the clipboard: {clipboard:?}"
            ));
        }
        Err(err) => failures.push(format!("cancel: {err}")),
    }

    verify_a_repeated_column_name_is_refused(&mut failures);
    verify_a_new_result_does_not_inherit_a_queued_export(&mut failures);
    verify_a_sql_inserts_export_writes_the_rows_it_started_with(&mut failures);
    verify_an_export_covers_the_result_the_user_asked_on(&mut main_window, &mut failures);
    verify_an_export_whose_result_closed_is_refused(&mut main_window, &mut failures);

    if failures.is_empty() {
        say("\nResult export verified end to end through the running application.");
    } else {
        eprintln!("\nFAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
    app::quit();
}

/// A result whose columns repeat a name cannot be written as SQL.
///
/// `SELECT a.id, b.id` is an ordinary query and every driver reports both
/// columns as `ID`. The grid's own reader has to say so BEFORE a snapshot is
/// taken, because two of the three shapes used to run: measured on Oracle 23ai,
/// `UPDATE t SET ID = 2 WHERE ID = 1` reported one row updated and
/// `WHERE ID = 1 AND ID = 2` matched none.
///
/// Driven on a real widget rather than in a unit test because the widget is
/// what turns a scope into the list of columns a statement would name, and that
/// is the step the refusal has to agree with.
fn verify_a_repeated_column_name_is_refused(failures: &mut Vec<String>) {
    let mut grid = ResultTableWidget::new();
    grid.start_streaming(&["ID".to_string(), "ID".to_string()]);
    grid.append_rows(vec![vec!["1".to_string(), "2".to_string()]]);
    grid.finish_streaming();

    match grid.sql_export_refusal(ExportScope::All) {
        Some(reason) if reason.contains("ID") => {
            say(&format!(
                "PASS: a repeated column name is refused — {reason}"
            ));
        }
        Some(reason) => failures.push(format!("the refusal does not name the column: {reason}")),
        None => failures
            .push("a result with two columns named ID was not refused for SQL export".to_string()),
    }

    // The same grid with distinct names is not refused, so the rule is about
    // the repetition and not about the grid.
    let mut plain = ResultTableWidget::new();
    plain.start_streaming(&["ID".to_string(), "NAME".to_string()]);
    plain.append_rows(vec![vec!["1".to_string(), "x".to_string()]]);
    plain.finish_streaming();
    if let Some(reason) = plain.sql_export_refusal(ExportScope::All) {
        failures.push(format!(
            "a result with distinct names was refused: {reason}"
        ));
    } else {
        say("PASS: a result whose column names are distinct is not refused");
    }
}

/// A `SQL Inserts` export writes the rows it was STARTED with.
///
/// It is the one format that cannot be finished where the rows are read: it
/// must not name a column the server computes, and only the catalog knows which
/// those are — a round trip. The grid therefore hands back a SNAPSHOT
/// (`ExportPayload::Sql`) and the script is built when the answer lands.
///
/// What must not happen in between is the grid changing the file. The road used
/// to re-resolve which grid to render when the catalog answered, and it asked
/// only whether that grid still showed the same TABLE — which a re-run of the
/// same query, another result tab on the same table, and a changed selection all
/// answer yes to. This drives the production snapshot on a real widget, replaces
/// the result underneath it, and builds from the snapshot afterwards.
fn verify_a_sql_inserts_export_writes_the_rows_it_started_with(failures: &mut Vec<String>) {
    let dialect = SqlWriteDialect::family_default(DatabaseType::MySQL);
    let mut grid = ResultTableWidget::new();
    grid.start_streaming(&["ID".to_string()]);
    grid.append_rows(vec![vec!["1".to_string()]]);
    grid.finish_streaming();
    grid.capture_tour_select_range(0, 0, 0, 0);

    // The production snapshot, taken exactly where the export road takes it.
    let Some(selection) = grid.sql_export_selection(dialect, Some("APP.T".to_string())) else {
        failures.push("the grid produced no SQL Inserts snapshot to export".to_string());
        return;
    };

    // The catalog read is in flight; the user re-runs the same query, and the
    // grid takes a new result of the SAME table.
    grid.start_streaming(&["ID".to_string()]);
    grid.append_rows(vec![vec!["999".to_string()]]);
    grid.finish_streaming();
    grid.capture_tour_select_range(0, 0, 0, 0);

    let built = build_sql_inserts(&selection);
    let text = built.text().to_string();
    // Either spelling of the value: a streamed grid reports no column kinds, so
    // the literal is quoted. What matters is WHICH row it holds.
    let wrote_its_own_row = text.contains("VALUES ('1')") || text.contains("VALUES (1)");
    if wrote_its_own_row && !text.contains("999") && built.rows() == 1 {
        say("PASS: a SQL Inserts export writes the rows it was started with");
    } else {
        failures.push(format!(
            "the export followed the grid to its new result instead of writing its own rows: \
             {text:?}"
        ));
    }
}

/// A queued export belongs to the result it was queued against.
///
/// An "All rows" export of a grid with an open lazy fetch queues itself behind
/// a full fetch. `start_streaming` — a NEW result landing in the same grid —
/// used to clear the lazy session and leave that queue alone, while
/// `clear_lazy_fetch_session` runs pending actions by session id without asking
/// whether the session is still the grid's. The export would then have rendered
/// the NEW result into the file the user named for the old one.
fn verify_a_new_result_does_not_inherit_a_queued_export(failures: &mut Vec<String>) {
    let mut grid = ResultTableWidget::new();
    grid.start_streaming(&["A".to_string()]);
    grid.append_rows(vec![vec!["old".to_string()]]);

    let fetch_all_asked = Arc::new(Mutex::new(false));
    let asked = fetch_all_asked.clone();
    // The production type's own shape: a lazy-fetch callback lives on the UI
    // thread and is never sent anywhere.
    #[allow(clippy::arc_with_non_send_sync)]
    let callback: LazyFetchCallback =
        Arc::new(Mutex::new(Some(Box::new(move |_session_id, _request| {
            *asked.lock().unwrap_or_else(|p| p.into_inner()) = true;
            true
        }))));
    grid.set_lazy_fetch_callback(callback);
    grid.set_lazy_fetch_session(4242);

    // The payload, not the finished bytes: a ready grid hands back what the
    // export will be built FROM, which for a data format is already its text.
    type Outcome = Result<ExportPayload, ExportAbandonReason>;
    let outcome: Arc<Mutex<Option<Outcome>>> = Arc::new(Mutex::new(None));
    let outcome_for_callback = outcome.clone();
    let queued = grid.capture_tour_export_after_fetch_all(Box::new(move |ready| {
        *outcome_for_callback
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(ready);
    }));
    if queued.is_some() {
        failures.push("the export did not queue behind the full fetch".to_string());
        return;
    }
    if !*fetch_all_asked.lock().unwrap_or_else(|p| p.into_inner()) {
        failures.push("queueing did not ask for the rest of the rows".to_string());
        return;
    }

    // A NEW result lands in the same grid.
    grid.start_streaming(&["B".to_string()]);
    grid.append_rows(vec![vec!["new".to_string()]]);
    grid.finish_streaming();

    match outcome.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        // And it is abandoned for the RIGHT reason: the notice the user reads
        // says a new result took the grid, not that the rows stopped arriving.
        Some(Err(ExportAbandonReason::ResultReplaced)) => {
            say("PASS: a queued export is abandoned when a new result replaces its own")
        }
        Some(Err(other)) => failures.push(format!(
            "the queued export was abandoned, but reported {other:?} rather than a replacement"
        )),
        Some(Ok(ExportPayload::Data(built))) => failures.push(format!(
            "the queued export rendered the NEW result: {} rows, {:?}",
            built.rows(),
            built.text()
        )),
        Some(Ok(ExportPayload::Sql(_))) => failures.push(
            "a CSV export handed back a SQL Inserts snapshot, which it can never build".to_string(),
        ),
        None => failures.push(
            "the queued export was neither run nor abandoned — the caller waits forever"
                .to_string(),
        ),
    }

    // And the completion of the old session must not resurrect it.
    grid.clear_lazy_fetch_session(4242, true);
    let after_close = outcome.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(Ok(payload)) = after_close {
        failures.push(format!(
            "closing the old session ran the abandoned export after all: {:?}",
            payload.data().map(|built| built.text().to_string())
        ));
    }
}

/// A result another statement of a running batch delivers — one distinctive
/// value, so a clipboard that followed the wrong grid names itself.
fn stolen_result() -> QueryResult {
    QueryResult {
        sql: "SELECT B FROM OTHER".into(),
        row_count: 1,
        execution_time: std::time::Duration::from_millis(3),
        message: "1 row selected".into(),
        is_select: true,
        success: true,
        columns: vec![column("B", SqlValueKind::String)],
        rows: vec![vec!["stolen".into()]],
    }
}

/// An export covers the result the user ASKED on, whatever the modals let
/// happen underneath them.
///
/// Between the click and the confirm, two modal loops run — the format dialog
/// and (for a file) the save chooser — and the app is live under both: a later
/// statement of a still-running batch creates and SELECTS its own result tab
/// through the same channel polls the modal pumps. The flow used to resolve
/// "which grid" through `current_table()` at the confirm, so the export
/// silently wrote whichever result was active by then. The grid is pinned by
/// tab id at the click now, and this drives the whole road — menu, modal, a
/// result landing under it, confirm — and reads the clipboard back.
fn verify_an_export_covers_the_result_the_user_asked_on(
    main_window: &mut MainWindow,
    failures: &mut Vec<String>,
) {
    // Both scopes: "All rows" proves the pin resolves the right GRID, and
    // "Selected rows" proves the pinned grid's own selection — widget-local
    // state — survives another tab taking the active slot.
    for scope in [ExportScope::All, ExportScope::Selection] {
        if let Err(err) =
            main_window.capture_tour_show_result("Result", sample_result(), false, None)
        {
            failures.push(format!(
                "mid-modal arrival ({scope:?}): could not show the fixture: {err}"
            ));
            return;
        }
        pump(400);
        if scope == ExportScope::Selection {
            main_window.capture_tour_select_result_range(0, 0, 1, 2);
        } else {
            main_window.capture_tour_clear_result_selection();
        }
        pump(200);

        set_clipboard("");
        {
            let mut plan = plan()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            plan.format = ExportFormat::Csv;
            plan.scope = scope;
            plan.cancel = false;
            plan.driven = false;
            plan.refused = false;
            plan.attempts = 0;
        }

        // Inside the modal's own event loop, before Export is clicked: a later
        // statement finishes and its tab takes the active slot.
        let arrival = main_window.capture_tour_result_arrival_handle();
        app::add_timeout3(0.15, move |_| arrival.land("Result 2", stolen_result()));
        app::add_timeout3(0.45, |_| drive_modal());
        if let Err(err) = trigger_export_menu() {
            failures.push(format!("mid-modal arrival ({scope:?}): {err}"));
            return;
        }
        pump(1500);

        if !plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .driven
        {
            failures.push(format!(
                "mid-modal arrival ({scope:?}): the export modal never appeared"
            ));
            return;
        }
        let clipboard = read_clipboard();
        let expected = render(ExportFormat::Csv, &expected_grid(scope));
        if clipboard == expected {
            say(&format!(
                "PASS: a {scope:?} export covers the result the user asked on, not the tab a \
                 batch selected"
            ));
        } else if clipboard.contains("stolen") {
            failures.push(format!(
                "the {scope:?} export followed the active tab to a result that arrived under \
                 the modal"
            ));
        } else {
            failures.push(format!(
                "mid-modal arrival ({scope:?}): clipboard held {clipboard:?}, expected the \
                 pinned result's CSV"
            ));
        }
    }
}

/// An export whose result is GONE by the confirm refuses loudly and writes
/// nothing.
///
/// The pinned grid resolves by tab id; a workspace that lost its results while
/// the dialogs were open answers with a sentence, never with whatever result
/// took its place — and never with a file that looks like a finished export.
fn verify_an_export_whose_result_closed_is_refused(
    main_window: &mut MainWindow,
    failures: &mut Vec<String>,
) {
    if let Err(err) = main_window.capture_tour_show_result("Result", sample_result(), false, None) {
        failures.push(format!(
            "closed-result export: could not show the fixture: {err}"
        ));
        return;
    }
    pump(400);
    main_window.capture_tour_clear_result_selection();
    pump(200);

    set_clipboard("sentinel-gone");
    {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.format = ExportFormat::Csv;
        plan.scope = ExportScope::All;
        plan.cancel = false;
        plan.driven = false;
        plan.refused = false;
        plan.attempts = 0;
    }

    // The workspace loses every result while the modal is open; another one
    // lands in its place, so "some grid is visible" cannot mask the loss.
    let arrival = main_window.capture_tour_result_arrival_handle();
    app::add_timeout3(0.15, move |_| {
        arrival.replace_all("Result 2", stolen_result());
    });
    app::add_timeout3(0.45, |_| drive_modal());
    // The refusal is an alert raised AFTER the export dialog closes; dismiss it
    // and record what it said.
    let alert_outcome: Arc<Mutex<(bool, bool)>> = Arc::new(Mutex::new((false, false)));
    {
        let alert_outcome = alert_outcome.clone();
        app::add_timeout3(0.60, move |_| {
            dismiss_result_gone_alert(alert_outcome.clone(), 0);
        });
    }
    if let Err(err) = trigger_export_menu() {
        failures.push(format!("closed-result export: {err}"));
        return;
    }
    pump(2000);

    let (seen, right_sentence) = *alert_outcome
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !seen {
        failures
            .push("the export of a result closed under the modal said nothing at all".to_string());
    } else if !right_sentence {
        failures
            .push("the closed-result refusal did not say the result is no longer open".to_string());
    } else {
        say("PASS: an export whose result closed under the modal refuses and says so");
    }
    let clipboard = read_clipboard();
    if clipboard != "sentinel-gone" {
        failures.push(format!(
            "the refused export still wrote to the clipboard: {clipboard:?}"
        ));
    } else {
        say("PASS: the refused export wrote nothing");
    }
}

/// Find the refusal alert, record whether it names the closed result, and
/// dismiss it so the run can go on.
fn dismiss_result_gone_alert(outcome: Arc<Mutex<(bool, bool)>>, attempts: u32) {
    let Some(alert) = window_by_label("Alert") else {
        if attempts < 60 {
            app::add_timeout3(0.05, move |_| {
                dismiss_result_gone_alert(outcome.clone(), attempts + 1);
            });
        }
        return;
    };
    let mut widgets = Vec::new();
    if let Some(group) = alert.as_group() {
        collect_widgets(&group, &mut widgets);
    }
    let right_sentence = widgets
        .iter()
        .any(|widget| widget.label().contains("no longer open"));
    *outcome
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = (true, right_sentence);
    for widget in &widgets {
        if let Some(mut button) = Button::from_dyn_widget(widget) {
            if button.label() == "Close" {
                button.do_callback();
                return;
            }
        }
    }
    let mut alert = alert;
    alert.hide();
}

/// Start an export from the application's own menu, drive the modal it opens,
/// and return what landed on the clipboard.
fn run_export(
    main_window: &mut MainWindow,
    format: ExportFormat,
    scope: ExportScope,
    cancel: bool,
) -> Result<String, String> {
    if scope == ExportScope::Selection {
        main_window.capture_tour_select_result_range(0, 0, 1, 2);
    } else {
        main_window.capture_tour_clear_result_selection();
    }
    pump(200);

    if !cancel {
        set_clipboard("");
    }
    {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.format = format;
        plan.scope = scope;
        plan.cancel = cancel;
        plan.driven = false;
        plan.refused = false;
        plan.attempts = 0;
    }

    app::add_timeout3(0.30, |_| drive_modal());
    trigger_export_menu()?;
    pump(1500);

    let (driven, refused) = {
        let plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (plan.driven, plan.refused)
    };
    if refused {
        return Err("the export refused with an alert instead of opening the modal".to_string());
    }
    if !driven {
        return Err("the export modal never appeared".to_string());
    }
    Ok(read_clipboard())
}

/// Fire the real `Tools > Export Results` menu item.
fn trigger_export_menu() -> Result<(), String> {
    let mut menu =
        app::widget_from_id::<MenuBar>("main_menu").ok_or_else(|| "no main menu".to_string())?;
    let index = menu.find_index(MENU_PATH);
    if index < 0 {
        return Err(format!("menu item {MENU_PATH} not found"));
    }
    menu.set_value(index);
    menu.do_callback();
    Ok(())
}

/// Set the modal's controls and click a button, from inside its own event loop.
fn drive_modal() {
    let Some(dialog) = window_by_label("Export Results") else {
        // Any other modal means the export refused before the dialog: dismiss
        // it so the run reports the refusal instead of hanging on it.
        if let Some(mut alert) = window_by_label("Alert") {
            plan()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .refused = true;
            alert.hide();
            return;
        }
        // The modal opens on a scheduled callback, so try again shortly, but
        // not forever.
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.attempts += 1;
        if plan.attempts > 40 {
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

    let mut plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    for widget in &widgets {
        if let Some(mut choice) = Choice::from_dyn_widget(widget) {
            plan.offered = (0..choice.size().saturating_sub(1))
                .filter_map(|index| choice.text(index))
                .collect();
            if let Some(index) = plan
                .offered
                .iter()
                .position(|label| label == plan.format.label())
            {
                choice.set_value(index as i32);
            }
        }
    }

    let wanted_scope = match plan.scope {
        ExportScope::All => "All rows",
        ExportScope::Selection => "Selected rows",
    };
    for widget in &widgets {
        let Some(mut radio) = RadioRoundButton::from_dyn_widget(widget) else {
            continue;
        };
        let label = radio.label();
        // Only "Clipboard" is wanted for the destination; the scope follows the
        // plan. Setting both members of a pair keeps FLTK's group state honest.
        match label.as_str() {
            "All rows" | "Selected rows" => radio.set_value(label == wanted_scope),
            "File" => radio.set_value(false),
            "Clipboard" => radio.set_value(true),
            _ => {}
        }
    }

    let wanted_button = if plan.cancel { "Cancel" } else { "Export" };
    plan.driven = true;
    drop(plan);

    for widget in &widgets {
        if let Some(mut button) = Button::from_dyn_widget(widget) {
            if button.label() == wanted_button {
                button.do_callback();
                return;
            }
        }
    }

    // Nothing to click: close the modal so the run does not hang.
    let mut dialog = dialog;
    dialog.hide();
    let _ = app::handle_main(Event::Push);
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

fn set_clipboard(text: &str) {
    app::copy(text);
    pump(20);
}

fn read_clipboard() -> String {
    // `pbpaste` reads the real OS clipboard, so this is what a paste would give.
    match Command::new("pbpaste").output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(_) => String::new(),
    }
}
