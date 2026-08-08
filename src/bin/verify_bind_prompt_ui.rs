#![allow(clippy::cargo, clippy::pedantic)]

// End-to-end verification of the bind-parameter modal's widget wiring.
//
// The scanning and substitution rules are covered by unit tests, which call
// `collect_bind_params` and `prepare` directly. Everything between the modal
// appearing and those functions being called was untested: whether a
// remembered answer really lands in the widgets, whether the type Choice and
// the value Input are read back onto the right parameter, whether the NULL
// checkbox both disables its row's input and reaches the caller, and whether
// Cancel gives back nothing at all.
//
// The modal here is the production one. Only the pointer is replaced: a
// timeout set before the modal opens edits its controls and clicks a button
// from inside the modal's own event loop, the way a user would.
//
// Usage: cargo run --bin verify_bind_prompt_ui

use fltk::{
    app,
    button::{Button, CheckButton},
    group::Group,
    input::Input,
    menu::Choice,
    prelude::*,
    window::Window,
};
use space_query::db::DatabaseType;
use space_query::ui::bind_prompt::{BindParam, BindParamType};
use space_query::ui::bind_prompt_dialog;
use space_query::utils::arithmetic::safe_div;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// What the timeout should do to each row before clicking a button.
#[derive(Clone)]
struct RowPlan {
    param_type: BindParamType,
    value: String,
    null: bool,
}

struct ModalPlan {
    rows: Vec<RowPlan>,
    cancel: bool,
    /// Values the modal showed before the timeout touched anything.
    seen_values: Vec<String>,
    /// Type labels the modal offered in its first Choice.
    offered_types: Vec<String>,
    /// Whether every value input whose row was set to NULL went inactive.
    null_deactivated: bool,
    /// Whether a `Ref Cursor` row disabled both its value field and its NULL
    /// box, since neither means anything for an OUT cursor.
    ref_cursor_deactivated: bool,
    driven: bool,
    attempts: u32,
}

static PLAN: OnceLock<Mutex<ModalPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<ModalPlan> {
    PLAN.get_or_init(|| {
        Mutex::new(ModalPlan {
            rows: Vec::new(),
            cancel: false,
            seen_values: Vec::new(),
            offered_types: Vec::new(),
            null_deactivated: true,
            ref_cursor_deactivated: true,
            driven: false,
            attempts: 0,
        })
    })
}

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

fn param(label: &str, bind_name: &str) -> BindParam {
    BindParam {
        label: label.to_string(),
        memo_key: bind_name.to_string(),
        bind_name: bind_name.to_string(),
        param_type: BindParamType::String,
        value: String::new(),
        is_null: false,
    }
}

/// Open the real modal, drive it, and return what it handed back.
fn run_modal(
    params: &[BindParam],
    rows: Vec<RowPlan>,
    cancel: bool,
    db_type: DatabaseType,
) -> Option<Vec<BindParam>> {
    {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.rows = rows;
        plan.cancel = cancel;
        plan.seen_values.clear();
        plan.offered_types.clear();
        plan.null_deactivated = true;
        plan.ref_cursor_deactivated = true;
        plan.driven = false;
        plan.attempts = 0;
    }
    app::add_timeout3(0.30, |_| drive_modal());
    bind_prompt_dialog::show(params, BindParamType::offered_for(db_type))
}

fn drive_modal() {
    let Some(dialog) = window_by_label("Bind Parameters") else {
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
    let Some(group) = dialog.as_group() else {
        return;
    };
    let mut widgets = Vec::new();
    collect_widgets(&group, &mut widgets);

    // The rows are built in order, so the n-th widget of each kind belongs to
    // the n-th parameter.
    let mut choices: Vec<Choice> = widgets.iter().filter_map(Choice::from_dyn_widget).collect();
    let mut inputs: Vec<Input> = widgets.iter().filter_map(Input::from_dyn_widget).collect();
    let mut checks: Vec<CheckButton> = widgets
        .iter()
        .filter_map(CheckButton::from_dyn_widget)
        .collect();

    let mut plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    plan.seen_values = inputs.iter().map(|input| input.value()).collect();
    if let Some(choice) = choices.first() {
        plan.offered_types = (0..choice.size().saturating_sub(1))
            .filter_map(|index| choice.text(index))
            .collect();
    }

    let rows = plan.rows.clone();
    for (index, row) in rows.iter().enumerate() {
        if let Some(choice) = choices.get_mut(index) {
            let wanted = BindParamType::ALL
                .iter()
                .position(|candidate| *candidate == row.param_type)
                .unwrap_or_default();
            choice.set_value(wanted as i32);
            // Setting a value by hand does not fire FLTK's callback, and the
            // callbacks are what enable and disable the rest of the row — so
            // invoke them the way a click would.
            choice.do_callback();
        }
        if let Some(input) = inputs.get_mut(index) {
            input.set_value(&row.value);
        }
        if let Some(check) = checks.get_mut(index) {
            check.set_value(row.null);
            check.do_callback();
        }
        if row.null {
            if let Some(input) = inputs.get(index) {
                if input.active() {
                    plan.null_deactivated = false;
                }
            }
        }
        if !row.param_type.takes_a_value() {
            let value_live = inputs.get(index).is_some_and(Input::active);
            let null_live = checks.get(index).is_some_and(CheckButton::active);
            if value_live || null_live {
                plan.ref_cursor_deactivated = false;
            }
        }
    }

    let wanted_button = if plan.cancel { "Cancel" } else { "Run" };
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

fn main() {
    let mut failures: Vec<String> = Vec::new();

    let _app = app::App::default();
    let mut host = Window::default().with_size(900, 600).with_label("host");
    host.end();
    host.show();
    pump(400);

    // 1. Two parameters, one typed by hand and one left prefilled.
    let mut prefilled = param(":ID", "ID");
    prefilled.param_type = BindParamType::Number;
    prefilled.value = "42".to_string();
    let params = vec![prefilled, param("? 1", "SQ_P1")];

    let answered = run_modal(
        &params,
        vec![
            RowPlan {
                param_type: BindParamType::Number,
                value: "42".to_string(),
                null: false,
            },
            RowPlan {
                param_type: BindParamType::Date,
                value: "2026-08-08".to_string(),
                null: false,
            },
        ],
        false,
        DatabaseType::Oracle,
    );
    pump(200);

    let (seen, offered, driven) = {
        let plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            plan.seen_values.clone(),
            plan.offered_types.clone(),
            plan.driven,
        )
    };
    if !driven {
        eprintln!("the bind modal never appeared");
        std::process::exit(1);
    }

    if seen.first().map(String::as_str) == Some("42") {
        say("PASS: a remembered answer is on screen when the modal opens");
    } else {
        failures.push(format!(
            "the modal opened showing {seen:?}, expected \"42\" first"
        ));
    }

    let expected_types: Vec<String> = BindParamType::offered_for(DatabaseType::Oracle)
        .iter()
        .map(|param_type| param_type.label().to_string())
        .collect();
    if offered == expected_types {
        say(&format!(
            "PASS: the type selector offers {}",
            offered.join(", ")
        ));
    } else {
        failures.push(format!(
            "the type selector offered {offered:?}, expected {expected_types:?}"
        ));
    }

    match answered {
        Some(answered) if answered.len() == 2 => {
            if answered[0].param_type == BindParamType::Number
                && answered[0].value == "42"
                && answered[0].bind_name == "ID"
            {
                say("PASS: row 1 came back as the Number 42 on bind ID");
            } else {
                failures.push(format!("row 1 came back as {:?}", answered[0]));
            }
            if answered[1].param_type == BindParamType::Date
                && answered[1].value == "2026-08-08"
                && answered[1].bind_name == "SQ_P1"
            {
                say("PASS: row 2 came back as the Date 2026-08-08 on bind SQ_P1");
            } else {
                failures.push(format!("row 2 came back as {:?}", answered[1]));
            }
        }
        Some(answered) => failures.push(format!(
            "the modal returned {} rows, expected 2",
            answered.len()
        )),
        None => failures.push("Run returned nothing".to_string()),
    }

    // 2. NULL must disable its row's input and reach the caller.
    let answered = run_modal(
        &[param(":A", "A")],
        vec![RowPlan {
            param_type: BindParamType::String,
            value: "ignored".to_string(),
            null: true,
        }],
        false,
        DatabaseType::Oracle,
    );
    pump(200);
    let deactivated = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .null_deactivated;
    if deactivated {
        say("PASS: checking NULL disables the value field on that row");
    } else {
        failures.push("checking NULL left the value field editable".to_string());
    }
    match answered {
        Some(answered) if answered.first().is_some_and(|param| param.is_null) => {
            say("PASS: a NULL row reaches the caller as NULL");
        }
        other => failures.push(format!("the NULL row came back as {other:?}")),
    }

    // 3. Cancel must give back nothing, so the statement does not run.
    let answered = run_modal(
        &[param(":A", "A")],
        vec![RowPlan {
            param_type: BindParamType::String,
            value: "typed".to_string(),
            null: false,
        }],
        true,
        DatabaseType::Oracle,
    );
    pump(200);
    if answered.is_none() {
        say("PASS: cancelling the modal returns no values at all");
    } else {
        failures.push(format!("cancel returned {answered:?}"));
    }

    // 4. A Ref Cursor row: an OUT cursor has no value to type, so both the
    //    value field and the NULL box must go dead.
    let answered = run_modal(
        &[param(":RC", "RC")],
        vec![RowPlan {
            param_type: BindParamType::RefCursor,
            value: String::new(),
            null: false,
        }],
        false,
        DatabaseType::Oracle,
    );
    pump(200);
    let deactivated = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .ref_cursor_deactivated;
    if deactivated {
        say("PASS: a Ref Cursor row disables its value field and its NULL box");
    } else {
        failures.push("a Ref Cursor row left its value or NULL control live".to_string());
    }
    match answered {
        Some(answered)
            if answered
                .first()
                .is_some_and(|param| param.param_type == BindParamType::RefCursor) =>
        {
            say("PASS: a Ref Cursor row reaches the caller as a Ref Cursor");
        }
        other => failures.push(format!("the Ref Cursor row came back as {other:?}")),
    }

    // 5. The MySQL family has no ref cursors, so the selector must not offer one.
    let _ = run_modal(
        &[param(":A", "A")],
        vec![RowPlan {
            param_type: BindParamType::String,
            value: "x".to_string(),
            null: false,
        }],
        false,
        DatabaseType::MySQL,
    );
    pump(200);
    let offered = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .offered_types
        .clone();
    let expected_mysql: Vec<String> = BindParamType::offered_for(DatabaseType::MySQL)
        .iter()
        .map(|param_type| param_type.label().to_string())
        .collect();
    if offered == expected_mysql {
        say(&format!(
            "PASS: the MySQL type selector offers {}",
            offered.join(", ")
        ));
    } else {
        failures.push(format!(
            "the MySQL type selector offered {offered:?}, expected {expected_mysql:?}"
        ));
    }

    if failures.is_empty() {
        say("\nBind parameter modal verified end to end.");
    } else {
        eprintln!("\nFAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
    app::quit();
}
