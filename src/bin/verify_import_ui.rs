#![allow(clippy::cargo, clippy::pedantic)]

// End-to-end verification of file → table import through the running
// application's own modal.
//
// `verify_import_live` proves the whole pipeline against real servers, but it
// calls the parsers and the script builder directly. Everything the user
// actually touches — the format selector, the header checkbox, the NULL-text
// field, the per-column mapping selectors, and the way all four are re-read
// when one of them changes — was untested.
//
// This drives the production modal. Nothing is stubbed: `show` is the same
// function the object browser calls, and the only thing replaced is the
// pointer. A timeout running inside the modal's own event loop sets its
// controls and clicks a button the way a user would, and the SQL that comes
// back is compared with what those settings should produce.
//
// Usage: cargo run --bin verify_import_ui

use fltk::{
    app,
    button::{Button, CheckButton},
    enums::Event,
    group::Group,
    input::Input,
    menu::Choice,
    prelude::*,
    window::Window,
};
use space_query::db::{DatabaseType, TableColumnDetail};
use space_query::ui::result_export::ExportFormat;
use space_query::ui::{MainWindow, ObjectBrowserWidget};
use space_query::utils::{arithmetic::safe_div, AppConfig};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const DIALOG_TITLE: &str = "Import Data from File";
const TABLE: &str = "HR.EMP";

/// What the timeout should do to the modal when it appears.
#[derive(Default)]
struct ModalPlan {
    /// Format label to select, if the default is not wanted.
    format: Option<&'static str>,
    /// Value for the "First row is a header" checkbox.
    header: Option<bool>,
    /// Value for the NULL-text field.
    null_text: Option<String>,
    /// `(mapping row, item label)` overrides applied after the reload.
    remap: Vec<(usize, String)>,
    /// A NULL text typed AFTER the remap, to prove a hand-made mapping is not
    /// thrown away by a keystroke that cannot change the file's columns.
    null_text_after_remap: Option<String>,
    cancel: bool,

    /// Filled in by the timeout, for the assertions.
    driven: bool,
    /// Set when Import refused and left the modal open.
    refused: bool,
    header_active: bool,
    null_active: bool,
    formats: Vec<String>,
    mapping_labels: Vec<String>,
    summary: String,
    attempts: u32,
}

static PLAN: OnceLock<Mutex<ModalPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<ModalPlan> {
    PLAN.get_or_init(|| Mutex::new(ModalPlan::default()))
}

fn detail(name: &str, data_type: &str, nullable: bool) -> TableColumnDetail {
    TableColumnDetail {
        name: name.to_string(),
        data_type: data_type.to_string(),
        data_length: 0,
        data_precision: None,
        data_scale: None,
        nullable,
        default_value: None,
        is_primary_key: false,
        is_generated: false,
        is_invisible: false,
    }
}

/// The table the import targets, as its catalog would describe it.
fn catalog() -> Vec<TableColumnDetail> {
    vec![
        detail("EMPNO", "NUMBER", false),
        detail("ENAME", "VARCHAR2", true),
        detail("HIREDATE", "DATE", true),
        detail("SAL", "NUMBER", true),
    ]
}

const CSV: &str = "EMPNO,ENAME,HIREDATE,SAL\n\
                   7369,SMITH,1980-12-17,800\n\
                   7499,NULL,1981-02-20,1600\n";
const HEADERLESS_CSV: &str = "7369,SMITH,1980-12-17,800\n";
const JSON: &str = "[{\"EMPNO\": 7369, \"ENAME\": \"SMITH\", \"HIREDATE\": \"1980-12-17\", \
                    \"SAL\": null}]";

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

    // The modal centres itself on the main window and is modal to it, so the
    // application has to be up before it can be opened at all.
    let mut main_window = MainWindow::new_with_config(AppConfig::default());
    main_window.setup_callbacks();
    main_window.show();
    pump(600);

    // (1) The default path: a CSV whose header names the table's columns.
    match run(CSV, ExportFormat::Csv, ModalPlan::default()) {
        Ok(Some((sql, summary))) => {
            let expected = "INSERT ALL\n\
                 \x20 INTO HR.EMP (EMPNO, ENAME, HIREDATE, SAL) VALUES \
                 (7369, 'SMITH', TO_DATE('1980-12-17','YYYY-MM-DD'), 800)\n\
                 \x20 INTO HR.EMP (EMPNO, ENAME, HIREDATE, SAL) VALUES \
                 (7499, NULL, TO_DATE('1981-02-20','YYYY-MM-DD'), 1600)\n\
                 SELECT * FROM DUAL;\n";
            if sql == expected {
                say("PASS: a CSV maps onto the table by name and the NULL text becomes NULL");
            } else {
                failures.push(format!(
                    "default CSV produced {sql:?}, expected {expected:?}"
                ));
            }
            if summary != "2 row(s) into 4 column(s) of HR.EMP" {
                failures.push(format!("default CSV summary was {summary:?}"));
            }
        }
        Ok(None) => failures.push("default CSV produced no script".to_string()),
        Err(err) => failures.push(format!("default CSV: {err}")),
    }

    // (2) Every format is on offer, and the two choices that mean nothing for a
    //     format are visibly dead rather than quietly ignored.
    let (formats, header_active, null_active) = state();
    let expected_formats: Vec<String> = ExportFormat::ALL
        .into_iter()
        .map(|format| format.label().to_string())
        .collect();
    if formats == expected_formats {
        say("PASS: the modal offers every export format for import");
    } else {
        failures.push(format!("the modal offered {formats:?}"));
    }
    if header_active && null_active {
        say("PASS: CSV keeps the header and NULL-text choices live");
    } else {
        failures.push("CSV deactivated a choice it needs".to_string());
    }

    // (3) JSON names its own columns and spells NULL itself, so both choices
    //     must go dead — and the file must still import.
    match run(
        JSON,
        ExportFormat::Csv,
        ModalPlan {
            format: Some("JSON"),
            ..ModalPlan::default()
        },
    ) {
        Ok(Some((sql, _))) => {
            let (_, header_active, null_active) = state();
            if header_active || null_active {
                failures.push("JSON left the header or NULL-text choice live".to_string());
            } else {
                say("PASS: switching to JSON deactivates the choices it does not use");
            }
            if sql.contains("(7369, 'SMITH', TO_DATE('1980-12-17','YYYY-MM-DD'), NULL)") {
                say("PASS: changing the format re-reads the file and rebuilds the mapping");
            } else {
                failures.push(format!("JSON produced {sql:?}"));
            }
        }
        Ok(None) => failures.push("JSON produced no script".to_string()),
        Err(err) => failures.push(format!("JSON: {err}")),
    }

    // (4) With no header the file's columns are positional, so they map by
    //     position rather than by a name they do not have.
    match run(
        HEADERLESS_CSV,
        ExportFormat::Csv,
        ModalPlan {
            header: Some(false),
            ..ModalPlan::default()
        },
    ) {
        Ok(Some((sql, _))) => {
            if sql.contains("INTO HR.EMP (EMPNO, ENAME, HIREDATE, SAL) VALUES (7369, 'SMITH', ") {
                say("PASS: a header-less file maps onto the table by position");
            } else {
                failures.push(format!("header-less CSV produced {sql:?}"));
            }
            let mapping = state_mapping();
            if mapping
                .first()
                .is_some_and(|label| label.starts_with("EMPNO"))
            {
                say("PASS: the mapping rows show the table column each file column feeds");
            } else {
                failures.push(format!("header-less mapping showed {mapping:?}"));
            }
        }
        Ok(None) => failures.push("header-less CSV produced no script".to_string()),
        Err(err) => failures.push(format!("header-less CSV: {err}")),
    }

    // (5b) A hand-made mapping survives a keystroke in the NULL text.
    //
    // Every reload rebuilt the mapping rows from the DEFAULT mapping, and a
    // reload runs on every keystroke in that field — so typing there silently
    // undid every target the user had chosen. The columns of the file do not
    // change when only the NULL text does.
    match run(
        CSV,
        ExportFormat::Csv,
        ModalPlan {
            remap: vec![(1, "(skip)".to_string())],
            null_text_after_remap: Some("\\N".to_string()),
            ..ModalPlan::default()
        },
    ) {
        Ok(Some((sql, _))) => {
            if sql.contains("(EMPNO, HIREDATE, SAL)") && !sql.contains("SMITH") {
                say("PASS: a hand-made mapping survives typing in the NULL text");
            } else {
                failures.push(format!(
                    "the NULL text reset the mapping; the script was {sql:?}"
                ));
            }
        }
        Ok(None) => failures.push("the mapping-preserving import produced nothing".to_string()),
        Err(err) => failures.push(err),
    }

    // (5) A column the user sends to (skip) must not appear in the script.
    match run(
        CSV,
        ExportFormat::Csv,
        ModalPlan {
            remap: vec![(1, "(skip)".to_string())],
            ..ModalPlan::default()
        },
    ) {
        Ok(Some((sql, summary))) => {
            if sql.contains("(EMPNO, HIREDATE, SAL)") && !sql.contains("SMITH") {
                say("PASS: a skipped file column is left out of the INSERT");
            } else {
                failures.push(format!("skipping ENAME produced {sql:?}"));
            }
            if !summary.contains("1 file column(s) skipped") {
                failures.push(format!("skip summary was {summary:?}"));
            }
        }
        Ok(None) => failures.push("skipping a column produced no script".to_string()),
        Err(err) => failures.push(format!("skip: {err}")),
    }

    // (6) A NULL text the file does not use leaves every value alone.
    match run(
        CSV,
        ExportFormat::Csv,
        ModalPlan {
            null_text: Some("\\N".to_string()),
            ..ModalPlan::default()
        },
    ) {
        Ok(Some((sql, _))) => {
            if sql.contains("(7499, 'NULL', ") {
                say("PASS: the NULL text is honoured literally, so `NULL` stays a string");
            } else {
                failures.push(format!("custom NULL text produced {sql:?}"));
            }
        }
        Ok(None) => failures.push("custom NULL text produced no script".to_string()),
        Err(err) => failures.push(format!("custom NULL text: {err}")),
    }

    // (7) Cancel imports nothing.
    match run(
        CSV,
        ExportFormat::Csv,
        ModalPlan {
            cancel: true,
            ..ModalPlan::default()
        },
    ) {
        Ok(None) => say("PASS: cancelling the modal imports nothing"),
        Ok(Some((sql, _))) => failures.push(format!("cancel still produced {sql:?}")),
        Err(err) => failures.push(format!("cancel: {err}")),
    }

    // (8) A file the chosen format cannot read says so in the modal instead of
    //     producing a script.
    match run("not json at all", ExportFormat::Json, ModalPlan::default()) {
        Ok(None) => {
            let summary = state_summary();
            if !state_refused() {
                failures.push("Import closed the modal instead of refusing".to_string());
            }
            if summary.contains("The JSON is not valid") {
                say("PASS: an unreadable file reports the parser's reason and imports nothing");
            } else {
                failures.push(format!("unreadable file reported {summary:?}"));
            }
        }
        Ok(Some((sql, _))) => failures.push(format!("unreadable file produced {sql:?}")),
        Err(err) => failures.push(format!("unreadable file: {err}")),
    }

    // A column name is not a menu grammar. FLTK parses `/` as a submenu and
    // fltk-rs splits `add_choice` on `|` before any escape can reach it, so a
    // target whose name held either used to become TWO entries — and the
    // mapping this modal returns is the item INDEX, so every later column
    // pointed at its neighbour. Driven through the production modal: what is
    // read back is the text of the item each mapping selector is SHOWING.
    let hostile_columns = vec![
        detail("A|B", "NUMBER", true),
        detail("C/D", "VARCHAR2", true),
        detail("E&F_G", "VARCHAR2", true),
        detail("PLAIN", "NUMBER", true),
    ];
    let hostile_csv = "A|B,C/D,E&F_G,PLAIN\n1,x,y,2\n";
    let hostile_names: Vec<String> = hostile_columns.iter().map(|c| c.name.clone()).collect();

    // (1) Every selector offers one entry per target and starts on its own.
    match run_against(
        hostile_csv,
        ExportFormat::Csv,
        ModalPlan::default(),
        hostile_columns.clone(),
    ) {
        Ok(Some((sql, _))) => {
            let labels = state_mapping();
            let mismatched: Vec<String> = labels
                .iter()
                .zip(hostile_names.iter())
                .filter(|(label, name)| !label.starts_with(name.as_str()))
                .map(|(label, name)| format!("{name} shows as {label:?}"))
                .collect();
            if labels.len() != hostile_names.len() {
                failures.push(format!(
                    "expected {} mapping selectors, the modal built {}: {labels:?}",
                    hostile_names.len(),
                    labels.len()
                ));
            } else if !mismatched.is_empty() {
                failures.push(format!(
                    "a mapping selector points at the wrong column: {}",
                    mismatched.join("; ")
                ));
            } else if !hostile_names.iter().all(|name| sql.contains(name.as_str())) {
                failures.push(format!("the script does not name every column: {sql:?}"));
            } else {
                say("PASS: a column named with `|`, `/`, `&` or `_` is ONE entry, showing itself");
            }
        }
        Ok(None) => failures.push("the hostile-name case produced no script".to_string()),
        Err(err) => failures.push(format!("hostile column names: {err}")),
    }

    // (2) And the user picks by what they SEE: send `PLAIN`'s values into the
    // column called `A|B`, choosing it by that label, and free `A|B`'s own row
    // so the mapping stays legal. If a name became two entries the label is not
    // there to pick — or picking it lands on its neighbour.
    match run_against(
        hostile_csv,
        ExportFormat::Csv,
        ModalPlan {
            remap: vec![
                (0, "(skip)".to_string()),
                (3, "A|B  ·  null ok".to_string()),
            ],
            ..ModalPlan::default()
        },
        hostile_columns.clone(),
    ) {
        Ok(Some((sql, _))) => {
            if sql.contains("(\"A|B\", \"C/D\", \"E&F_G\") VALUES (2, 'x', 'y')") {
                say("PASS: picking a hostile name by its label lands on that column");
            } else {
                failures.push(format!(
                    "picking `A|B` by its label did not land on that column: {sql:?}"
                ));
            }
        }
        Ok(None) => failures.push("the remapped hostile-name case produced no script".to_string()),
        Err(err) => failures.push(format!("hostile column remap: {err}")),
    }

    // The helper installs a per-item callback, because `MenuExt::add` always
    // does. FLTK dispatches a pick as
    // `if (value_->callback_) value_->do_callback(this); else do_callback();`
    // (`Fl_Menu_.cxx`), so an item callback SUPPRESSES the widget's — and every
    // picker here sets its callback on the widget. `MenuItem::do_callback` is
    // that same branch, so this drives what a real pick drives.
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        let fired = Rc::new(RefCell::new(0usize));
        let fired_for_widget = fired.clone();
        let mut choice = Choice::new(0, 0, 10, 10, None);
        space_query::ui::widget_label::add_menu_item(&mut choice, "A|B");
        space_query::ui::widget_label::add_menu_item(&mut choice, "PLAIN");
        choice.set_callback(move |_| *fired_for_widget.borrow_mut() += 1);
        for index in 0..2 {
            if let Some(mut item) = choice.at(index) {
                item.do_callback(&choice);
            }
        }
        let count = *fired.borrow();
        if count == 2 {
            say("PASS: picking an item still runs the picker's own callback");
        } else {
            failures.push(format!(
                "a menu item added by the helper ran the widget callback {count} times, not 2 \
                 — an item callback is swallowing the picker's"
            ));
        }
    }

    if failures.is_empty() {
        say("\nImport verified end to end through the production modal.");
    } else {
        eprintln!("\nFAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
    app::quit();
}

/// Open the production modal over `text`, drive it, and return the script.
fn run(
    text: &str,
    format: ExportFormat,
    wanted: ModalPlan,
) -> Result<Option<(String, String)>, String> {
    run_against(text, format, wanted, catalog())
}

/// The same, against a catalog the caller chooses.
fn run_against(
    text: &str,
    format: ExportFormat,
    mut wanted: ModalPlan,
    columns: Vec<TableColumnDetail>,
) -> Result<Option<(String, String)>, String> {
    wanted.driven = false;
    wanted.refused = false;
    wanted.attempts = 0;
    *plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = wanted;

    app::add_timeout3(0.20, |_| drive_modal());
    // Import refuses an unreadable file with an alert that runs its own modal
    // loop inside the button callback. FLTK only arms a timeout added during a
    // timeout pass once that pass finishes, and the pass cannot finish while
    // the alert is up — so the dismissal has to be armed from out here, before
    // `drive_modal` ever runs.
    app::add_timeout3(0.45, |_| dismiss_alert(0));
    // Import that refuses leaves the modal open, which is the right thing for a
    // user and a hang for this driver. Close it once the click has had its say.
    app::add_timeout3(0.90, |_| close_if_refused(0));
    let outcome = ObjectBrowserWidget::build_import_script_from_dialog(
        "sample",
        text,
        TABLE,
        space_query::ui::grid_sql_export::SqlWriteDialect::family_default(DatabaseType::Oracle),
        &columns,
        format,
    );
    pump(200);

    if !plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .driven
    {
        return Err("the import modal never appeared".to_string());
    }
    Ok(outcome)
}

fn state() -> (Vec<String>, bool, bool) {
    let plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (plan.formats.clone(), plan.header_active, plan.null_active)
}

fn state_mapping() -> Vec<String> {
    plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .mapping_labels
        .clone()
}

fn state_refused() -> bool {
    plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .refused
}

fn state_summary() -> String {
    plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .summary
        .clone()
}

/// Set the modal's controls and click a button, from inside its own event loop.
fn drive_modal() {
    let Some(dialog) = window_by_label(DIALOG_TITLE) else {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.attempts += 1;
        if plan.attempts > 60 {
            return;
        }
        drop(plan);
        app::add_timeout3(0.05, |_| drive_modal());
        return;
    };

    // The format, header, and NULL-text controls each rebuild the mapping rows,
    // so they are set first and the rows are read afterwards.
    let (format, header, null_text) = {
        let plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (plan.format, plan.header, plan.null_text.clone())
    };

    if let Some(label) = format {
        if let Some(mut choice) = nth_choice(&dialog, 0) {
            let index = (0..choice.size().saturating_sub(1))
                .find(|index| choice.text(*index).as_deref() == Some(label));
            if let Some(index) = index {
                choice.set_value(index);
                choice.do_callback();
            }
        }
    }
    if let Some(header) = header {
        if let Some(mut check) = first_widget::<CheckButton>(&dialog) {
            check.set_value(header);
            check.do_callback();
        }
    }
    if let Some(null_text) = null_text {
        if let Some(mut input) = first_widget::<Input>(&dialog) {
            input.set_value(&null_text);
            input.do_callback();
        }
    }

    let remap = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remap
        .clone();
    for (row, label) in remap {
        // Row 0 is the format selector, so the mapping rows start at 1.
        if let Some(mut choice) = nth_choice(&dialog, row + 1) {
            let index = (0..choice.size().saturating_sub(1))
                .find(|index| choice.text(*index).as_deref() == Some(label.as_str()));
            if let Some(index) = index {
                choice.set_value(index);
                choice.do_callback();
            }
        }
    }

    // Typed AFTER the mapping was set by hand: the file's columns do not change
    // when only the NULL text does, so the rows on screen — and the targets the
    // user chose in them — have to survive it.
    let null_text_after_remap = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .null_text_after_remap
        .clone();
    if let Some(null_text) = null_text_after_remap {
        if let Some(mut input) = first_widget::<Input>(&dialog) {
            input.set_value(&null_text);
            input.do_callback();
        }
    }

    let choices = all_choices(&dialog);
    let mut plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(format_choice) = choices.first() {
        plan.formats = (0..format_choice.size().saturating_sub(1))
            .filter_map(|index| format_choice.text(index))
            .collect();
    }
    plan.mapping_labels = choices
        .iter()
        .skip(1)
        .map(|choice| choice.text(choice.value()).unwrap_or_default())
        .collect();
    plan.header_active = first_widget::<CheckButton>(&dialog).is_some_and(|w| w.active());
    plan.null_active = first_widget::<Input>(&dialog).is_some_and(|w| w.active());
    plan.summary = summary_text(&dialog);
    plan.driven = true;
    let cancel = plan.cancel;
    drop(plan);

    let wanted_button = if cancel { "Cancel" } else { "Import" };
    for widget in widgets(&dialog) {
        if let Some(mut button) = Button::from_dyn_widget(&widget) {
            if button.label() == wanted_button {
                button.do_callback();
                return;
            }
        }
    }

    let mut dialog = dialog;
    dialog.hide();
    let _ = app::handle_main(Event::Push);
}

/// Cancel the modal if Import refused and left it open.
fn close_if_refused(attempts: u32) {
    let Some(dialog) = window_by_label(DIALOG_TITLE) else {
        return;
    };
    if !plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .driven
    {
        if attempts < 40 {
            app::add_timeout3(0.05, move |_| close_if_refused(attempts + 1));
        }
        return;
    }
    plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .refused = true;
    for widget in widgets(&dialog) {
        if let Some(mut button) = Button::from_dyn_widget(&widget) {
            if button.label() == "Cancel" {
                button.do_callback();
                return;
            }
        }
    }
    let mut dialog = dialog;
    dialog.hide();
}

/// Close an alert the Import button raised, and stop looking once the import
/// modal itself is gone.
fn dismiss_alert(attempts: u32) {
    if let Some(alert) = window_by_label("Alert") {
        for widget in widgets(&alert) {
            if let Some(mut button) = Button::from_dyn_widget(&widget) {
                if button.label() == "Close" {
                    button.do_callback();
                    return;
                }
            }
        }
        let mut alert = alert;
        alert.hide();
        return;
    }
    if attempts < 20 && window_by_label(DIALOG_TITLE).is_some() {
        app::add_timeout3(0.05, move |_| dismiss_alert(attempts + 1));
    }
}

/// The summary line: the last frame in the dialog that carries a row count or a
/// parser error, rather than a fixed label.
fn summary_text(dialog: &Window) -> String {
    widgets(dialog)
        .iter()
        .filter_map(|widget| {
            let label = widget.label();
            (label.contains("row(s)") || label.contains("The ")).then_some(label)
        })
        .next_back()
        .unwrap_or_default()
}

fn widgets(dialog: &Window) -> Vec<fltk::widget::Widget> {
    let mut out = Vec::new();
    if let Some(group) = dialog.as_group() {
        collect_widgets(&group, &mut out);
    }
    out
}

fn collect_widgets(group: &Group, out: &mut Vec<fltk::widget::Widget>) {
    for child in group.clone().into_iter() {
        if let Some(child_group) = child.as_group() {
            collect_widgets(&child_group, out);
        }
        out.push(child);
    }
}

/// Every `Choice` in the dialog, in creation order: the format selector first,
/// then one per file column.
fn all_choices(dialog: &Window) -> Vec<Choice> {
    let mut choices: Vec<Choice> = widgets(dialog)
        .iter()
        .filter_map(Choice::from_dyn_widget)
        .collect();
    // `collect_widgets` reports a group's children before the group itself, and
    // the mapping rows sit in a Scroll after the format row; sorting by screen
    // position gives the order a user sees.
    choices.sort_by_key(WidgetExt::y);
    choices
}

fn nth_choice(dialog: &Window, index: usize) -> Option<Choice> {
    all_choices(dialog).into_iter().nth(index)
}

fn first_widget<W: WidgetBase>(dialog: &Window) -> Option<W> {
    widgets(dialog)
        .iter()
        .find_map(|widget| W::from_dyn_widget(widget))
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
