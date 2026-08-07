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
use space_query::db::{ColumnInfo, QueryResult, SqlValueKind};
use space_query::ui::result_export::{render, ExportFormat, ExportGrid, ExportScope};
use space_query::ui::MainWindow;
use space_query::utils::{arithmetic::safe_div, AppConfig};
use std::io::Write;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

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
        rows,
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
            failures.push(format!("cancel still wrote to the clipboard: {clipboard:?}"));
        }
        Err(err) => failures.push(format!("cancel: {err}")),
    }

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
        let mut plan = plan().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let plan = plan().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut plan = plan().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    let mut plan = plan().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

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
