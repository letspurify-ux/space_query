#![allow(clippy::cargo, clippy::pedantic)]

// End-to-end verification of the three conveniences added together, through the
// running application rather than through their pure helpers:
//
//   * Soft Wrap (item 28)      — Edit > Soft Wrap
//   * Go to Line (item 28)     — Edit > Go to Line / Ctrl+G
//   * Go to Object (item 10)   — Query > Go to Object / Ctrl+Shift+N
//   * Go to Declaration (10)   — Query > Go to Declaration / Ctrl+B
//
// The unit tests prove the parsing and the ranking. What they cannot prove is
// that the menu dispatch reaches them, that FLTK really changed its wrap mode
// (rather than being told to), that the setting reaches *every* editor tab and
// survives a restart, and that the search modal's own event loop returns what
// the list shows.
//
// Nothing is stubbed. The real `MainWindow` runs with its real callbacks, the
// actions start from the application's own menu bar, and the modals that open
// are the production ones — only the pointer is replaced, by a timeout that
// drives the modal from inside its own event loop.
//
// Usage: cargo run --bin verify_editor_convenience_ui

use fltk::{
    app, browser::HoldBrowser, button::Button, group::Group, input::Input, menu::MenuBar,
    prelude::*, window::Window,
};
use space_query::db::PackageRoutine;
use space_query::ui::object_browser::ObjectCache;
use space_query::ui::object_search_dialog;
use space_query::ui::MainWindow;
use space_query::utils::{arithmetic::safe_div, AppConfig};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const SOFT_WRAP_MENU: &str = "&Edit/Soft &Wrap";
const GO_TO_LINE_MENU: &str = "&Edit/&Go to Line";
const GO_TO_OBJECT_MENU: &str = "&Query/Go to &Object";
const GO_TO_DECLARATION_MENU: &str = "&Query/&Go to Declaration";

/// A line long enough that no sane window can draw it in one row.
const LONG_LINE: &str = "SELECT * FROM ORDERS WHERE ID IN (\
1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,\
31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,\
58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84)";

/// What a timeout should type into whichever modal appears.
struct ModalPlan {
    /// Text to put in the modal's only input.
    input: String,
    /// Label of the button to click.
    button: &'static str,
    /// Set once the timeout found and drove the modal.
    driven: bool,
    attempts: u32,
}

static PLAN: OnceLock<Mutex<ModalPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<ModalPlan> {
    PLAN.get_or_init(|| {
        Mutex::new(ModalPlan {
            input: String::new(),
            button: "OK",
            driven: false,
            attempts: 0,
        })
    })
}

fn arm_modal(input: &str, button: &'static str) {
    let mut plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    plan.input = input.to_string();
    plan.button = button;
    plan.driven = false;
    plan.attempts = 0;
}

fn modal_was_driven() -> bool {
    plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .driven
}

/// Fill the modal's input and click its button, from inside its event loop.
fn drive_modal(label: &'static str) {
    let Some(dialog) = window_by_label(label) else {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.attempts += 1;
        if plan.attempts > 60 {
            return;
        }
        drop(plan);
        app::add_timeout3(0.05, move |_| drive_modal(label));
        return;
    };

    let Some(group) = dialog.as_group() else {
        return;
    };
    let mut widgets = Vec::new();
    collect_widgets(&group, &mut widgets);

    let wanted_input = {
        let plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.input.clone()
    };
    if !wanted_input.is_empty() {
        for widget in &widgets {
            if let Some(mut input) = Input::from_dyn_widget(widget) {
                input.set_value(&wanted_input);
                input.do_callback();
                break;
            }
        }
    }

    // Read the list the modal is showing *now*, after the filter ran — that is
    // what the user would be looking at when they press the button.
    for widget in &widgets {
        if let Some(browser) = HoldBrowser::from_dyn_widget(widget) {
            let lines: Vec<String> = (1..=browser.size())
                .filter_map(|line| browser.text(line))
                .collect();
            LISTED
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend(lines);
            break;
        }
    }

    let wanted_button = {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.driven = true;
        plan.button
    };

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

fn check(failures: &mut Vec<String>, ok: bool, message: &str) {
    if ok {
        say(&format!("PASS: {message}"));
    } else {
        say(&format!("FAIL: {message}"));
        failures.push(message.to_string());
    }
}

/// Fire a real menu item. For a Toggle item the click flips it first, which is
/// what FLTK does before it calls the callback.
fn fire_menu(path: &str, toggle: Option<bool>) -> Result<(), String> {
    let mut menu =
        app::widget_from_id::<MenuBar>("main_menu").ok_or_else(|| "no main menu".to_string())?;
    let index = menu.find_index(path);
    if index < 0 {
        return Err(format!("menu item {path} not found"));
    }
    if let Some(checked) = toggle {
        if let Some(mut item) = menu.at(index) {
            if checked {
                item.set();
            } else {
                item.clear();
            }
        }
    }
    menu.set_value(index);
    menu.do_callback();
    Ok(())
}

fn menu_item_is_checked(path: &str) -> bool {
    app::widget_from_id::<MenuBar>("main_menu")
        .and_then(|menu| menu.find_item(path))
        .is_some_and(|item| item.value())
}

fn sample_cache() -> ObjectCache {
    let mut package_routines = HashMap::new();
    package_routines.insert(
        "PKG_ORDERS".to_string(),
        vec![PackageRoutine {
            name: "PLACE_ORDER".to_string(),
            routine_type: "PROCEDURE".to_string(),
        }],
    );
    ObjectCache {
        tables: vec!["ORDERS".to_string(), "ORDER_ITEMS".to_string()],
        views: vec!["V_ORDER_TOTALS".to_string()],
        procedures: vec!["REBUILD_ORDERS".to_string()],
        functions: vec!["ORDER_COUNT".to_string()],
        sequences: vec!["ORDER_SEQ".to_string()],
        triggers: Vec::new(),
        events: Vec::new(),
        synonyms: Vec::new(),
        packages: vec!["PKG_ORDERS".to_string()],
        package_routines,
    }
}

fn main() {
    let mut failures: Vec<String> = Vec::new();

    let _app = app::App::default();
    let config = AppConfig {
        editor_soft_wrap: false,
        ..AppConfig::default()
    };
    AppConfig::update_runtime(&config);
    let mut main_window = MainWindow::new_with_config(config);
    main_window.setup_callbacks();
    main_window.show();
    pump(600);

    // ---- Soft Wrap --------------------------------------------------------
    main_window.capture_tour_set_sql(LONG_LINE, Some(0));
    pump(300);
    // `count_lines` counts the line breaks between two positions, so a single
    // unwrapped line spans none.
    let unwrapped = main_window.capture_tour_editor_display_line_count();
    check(
        &mut failures,
        unwrapped == 0,
        &format!("one long line spans {unwrapped} extra rows before Soft Wrap"),
    );

    if let Err(err) = fire_menu(SOFT_WRAP_MENU, Some(true)) {
        say(&format!("FAIL: {err}"));
        failures.push(err);
    }
    pump(400);
    let wrapped = main_window.capture_tour_editor_display_line_count();
    check(
        &mut failures,
        wrapped > unwrapped,
        &format!("Soft Wrap makes the same line draw as {wrapped} rows"),
    );

    // A second tab created *after* the toggle has to come up wrapped too.
    main_window.capture_tour_new_editor_tab();
    pump(400);
    main_window.capture_tour_set_sql(LONG_LINE, Some(0));
    pump(300);
    let per_tab = main_window.capture_tour_all_editor_display_line_counts();
    check(
        &mut failures,
        main_window.capture_tour_editor_tab_count() == 2 && per_tab.iter().all(|count| *count > 1),
        &format!("Soft Wrap reaches every editor tab, old and new: {per_tab:?}"),
    );

    check(
        &mut failures,
        AppConfig::runtime().editor_soft_wrap,
        "Soft Wrap is written to the config, so it survives a restart",
    );
    check(
        &mut failures,
        menu_item_is_checked(SOFT_WRAP_MENU),
        "the Soft Wrap menu item shows its checked state",
    );

    if let Err(err) = fire_menu(SOFT_WRAP_MENU, Some(false)) {
        say(&format!("FAIL: {err}"));
        failures.push(err);
    }
    pump(400);
    let unwrapped_again = main_window.capture_tour_all_editor_display_line_counts();
    check(
        &mut failures,
        unwrapped_again.iter().all(|count| *count == 0) && !AppConfig::runtime().editor_soft_wrap,
        &format!("turning Soft Wrap off restores every tab: {unwrapped_again:?}"),
    );

    // ---- Go to Line -------------------------------------------------------
    let numbered: String = (1..=12)
        .map(|line| format!("-- line {line}\n"))
        .collect::<String>();
    main_window.capture_tour_set_sql(&numbered, Some(0));
    pump(300);

    arm_modal("7", "OK");
    app::add_timeout3(0.20, |_| drive_modal("Input"));
    if let Err(err) = fire_menu(GO_TO_LINE_MENU, None) {
        say(&format!("FAIL: {err}"));
        failures.push(err);
    }
    pump(600);
    check(
        &mut failures,
        modal_was_driven(),
        "Go to Line opens the input modal",
    );
    let caret_line = main_window.capture_tour_editor_caret_line();
    check(
        &mut failures,
        caret_line == 7,
        &format!("Go to Line 7 puts the caret on line {caret_line}"),
    );

    // Out of range clamps to the last line rather than refusing.
    arm_modal("999", "OK");
    app::add_timeout3(0.20, |_| drive_modal("Input"));
    let _ = fire_menu(GO_TO_LINE_MENU, None);
    pump(600);
    let clamped = main_window.capture_tour_editor_caret_line();
    check(
        &mut failures,
        clamped == 13,
        &format!("a line past the end clamps to the last line ({clamped})"),
    );

    // Cancel must not move the caret.
    arm_modal("2", "Cancel");
    app::add_timeout3(0.20, |_| drive_modal("Input"));
    let _ = fire_menu(GO_TO_LINE_MENU, None);
    pump(600);
    let after_cancel = main_window.capture_tour_editor_caret_line();
    check(
        &mut failures,
        after_cancel == clamped,
        &format!("cancelling Go to Line leaves the caret where it was ({after_cancel})"),
    );

    // ---- Go to Object / Go to Declaration without a connection ------------
    let tabs_before = main_window.capture_tour_editor_tab_count();
    let _ = fire_menu(GO_TO_OBJECT_MENU, None);
    pump(400);
    let _ = fire_menu(GO_TO_DECLARATION_MENU, None);
    pump(400);
    check(
        &mut failures,
        main_window.capture_tour_editor_tab_count() == tabs_before
            && window_by_label("Go to Object").is_none(),
        "without a connection both object actions report and open nothing",
    );

    // ---- the search modal itself -----------------------------------------
    let cache = sample_cache();

    arm_modal("order_c", "Open");
    app::add_timeout3(0.25, |_| drive_modal("Go to Object — SCOTT"));
    let picked = object_search_dialog::show(&cache, Some("SCOTT"));
    pump(200);
    let listed = LISTED
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    check(
        &mut failures,
        listed.iter().any(|line| line.starts_with("ORDER_COUNT")),
        &format!("typing filters the list to matching objects: {listed:?}"),
    );
    check(
        &mut failures,
        listed.iter().all(|line| line.contains('\t')),
        "each row carries a name and a kind",
    );
    check(
        &mut failures,
        picked
            .as_ref()
            .is_some_and(|hit| hit.display_name == "ORDER_COUNT"),
        &format!(
            "Open returns the highlighted object: {:?}",
            picked.as_ref().map(|hit| hit.display_name.clone())
        ),
    );

    arm_modal("orders", "Cancel");
    app::add_timeout3(0.25, |_| drive_modal("Go to Object"));
    let cancelled = object_search_dialog::show(&cache, None);
    pump(200);
    check(
        &mut failures,
        cancelled.is_none(),
        "Cancel returns nothing and opens nothing",
    );

    println!();
    if failures.is_empty() {
        say("ALL CHECKS PASSED");
    } else {
        say("FAILURES:");
        for failure in &failures {
            say(&format!("  - {failure}"));
        }
        std::process::exit(1);
    }
}

static LISTED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
