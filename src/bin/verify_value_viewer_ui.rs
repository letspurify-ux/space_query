#![allow(clippy::cargo, clippy::pedantic)]

// End-to-end verification of the cell value window's widget wiring.
//
// The formatters are covered by unit tests that call `format_json` and
// `format_xml` directly. What those cannot show is anything between the window
// appearing and a value coming back: whether a read-only value really gets a
// display-only widget and a single button, whether the Format checkbox is a
// *view* (so clearing it restores the exact bytes being edited, and saving
// while it is on writes the raw value rather than the indented one), and
// whether Save and Cancel hand back what they claim to.
//
// That Format rule is the one worth proving by driving the real widgets: if it
// were ever wrong, opening a CLOB and pressing Format to read it would silently
// rewrite the whitespace in the database on the next save.
//
// The window here is the production one. Only the pointer is replaced: a
// timeout set before it opens edits its controls and clicks a button from
// inside its own event loop, the way a user would.
//
// Usage: cargo run --bin verify_value_viewer_ui

use fltk::{
    app,
    button::{Button, CheckButton},
    group::Group,
    prelude::*,
    text::{TextDisplay, TextEditor},
    window::Window,
};
use space_query::ui::value_viewer;
use space_query::ui::{profile_by_name, value_viewer::ValueFormat};
use space_query::utils::arithmetic::safe_div;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const TITLE: &str = "Cell Value";

/// What the timeout should do to the window before clicking a button.
#[derive(Clone, Default)]
struct WindowPlan {
    /// Replace the buffer text with this before anything else.
    typed: Option<String>,
    /// Tick Format, look, and (when `unformat` is set) untick it again.
    format: bool,
    unformat: bool,
    /// Label of the button to click. `None` closes the window outright.
    button: Option<&'static str>,

    // -- what the timeout saw --
    buttons_seen: Vec<String>,
    /// True when the value sits in a `TextEditor` (editable), false when it
    /// sits in a `TextDisplay` (read-only).
    editor_present: bool,
    display_present: bool,
    format_active: bool,
    text_while_formatted: String,
    text_after_unformat: String,
    editor_active_while_formatted: bool,
    driven: bool,
    attempts: u32,
}

static PLAN: OnceLock<Mutex<WindowPlan>> = OnceLock::new();

fn plan() -> &'static Mutex<WindowPlan> {
    PLAN.get_or_init(|| Mutex::new(WindowPlan::default()))
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

/// Open the real window, drive it, and return what it handed back.
fn run_window(value: &str, editable: bool, actions: WindowPlan) -> (Option<String>, WindowPlan) {
    {
        let mut current = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *current = actions;
    }
    app::add_timeout3(0.30, |_| drive_window());
    let profile = profile_by_name("Courier");
    let outcome = value_viewer::show(TITLE, value, editable, profile, 14);
    pump(150);
    let seen = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    (outcome, seen)
}

fn drive_window() {
    let Some(dialog) = window_by_label(TITLE) else {
        let mut plan = plan()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.attempts += 1;
        if plan.attempts > 60 {
            return;
        }
        drop(plan);
        app::add_timeout3(0.05, |_| drive_window());
        return;
    };
    let Some(group) = dialog.as_group() else {
        return;
    };
    let mut widgets = Vec::new();
    collect_widgets(&group, &mut widgets);

    let editor: Option<TextEditor> = widgets.iter().find_map(TextEditor::from_dyn_widget);
    let display: Option<TextDisplay> = widgets
        .iter()
        .find_map(TextDisplay::from_dyn_widget)
        .filter(|_| editor.is_none());
    let mut format_check: Option<CheckButton> = widgets
        .iter()
        .filter_map(CheckButton::from_dyn_widget)
        .find(|check| check.label().trim_start().starts_with("Format"));
    // A CheckButton is a Button in FLTK's hierarchy, so the Format box would
    // otherwise show up in the list of action buttons.
    let buttons: Vec<Button> = widgets
        .iter()
        .filter(|widget| CheckButton::from_dyn_widget(*widget).is_none())
        .filter_map(Button::from_dyn_widget)
        .collect();

    let mut plan = plan()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    plan.editor_present = editor.is_some();
    plan.display_present = display.is_some();
    plan.buttons_seen = buttons
        .iter()
        .map(Button::label)
        .filter(|label| !label.is_empty())
        .collect();
    plan.format_active = format_check.as_ref().is_some_and(CheckButton::active);

    let mut buffer = editor
        .as_ref()
        .and_then(TextEditor::buffer)
        .or_else(|| display.as_ref().and_then(TextDisplay::buffer));

    if let (Some(text), Some(buffer)) = (plan.typed.clone(), buffer.as_mut()) {
        buffer.set_text(&text);
    }

    if plan.format {
        if let Some(check) = format_check.as_mut() {
            check.set_checked(true);
            check.do_callback();
        }
        plan.text_while_formatted = buffer
            .as_ref()
            .map(fltk::text::TextBuffer::text)
            .unwrap_or_default();
        plan.editor_active_while_formatted = editor.as_ref().is_some_and(|widget| widget.active());
        if plan.unformat {
            if let Some(check) = format_check.as_mut() {
                check.set_checked(false);
                check.do_callback();
            }
            plan.text_after_unformat = buffer
                .as_ref()
                .map(fltk::text::TextBuffer::text)
                .unwrap_or_default();
        }
    }

    let wanted = plan.button;
    plan.driven = true;
    drop(plan);

    if let Some(wanted) = wanted {
        for button in &buttons {
            if button.label() == wanted {
                let mut button = button.clone();
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

const JSON_VALUE: &str = r#"{"id":1,"tags":["a","b"],"note":"keep  spacing"}"#;

fn main() {
    let mut failures: Vec<String> = Vec::new();

    let _app = app::App::default();
    let mut host = Window::default().with_size(900, 600).with_label("host");
    host.end();
    host.show();
    pump(400);

    // 1. A read-only value gets a display widget and exactly one button.
    let (outcome, seen) = run_window(
        JSON_VALUE,
        false,
        WindowPlan {
            button: Some("Close"),
            ..WindowPlan::default()
        },
    );
    if seen.display_present && !seen.editor_present {
        say("PASS: a read-only value opens in a display-only widget");
    } else {
        failures.push(format!(
            "a read-only value opened with editor={} display={}",
            seen.editor_present, seen.display_present
        ));
    }
    if seen.buttons_seen == vec!["Close".to_string()] {
        say("PASS: a read-only value offers only Close");
    } else {
        failures.push(format!(
            "a read-only value offered buttons {:?}",
            seen.buttons_seen
        ));
    }
    if outcome.is_none() {
        say("PASS: closing a read-only value stages nothing");
    } else {
        failures.push(format!("closing a read-only value returned {outcome:?}"));
    }

    // 2. An editable value gets an editor, Save and Cancel.
    let (outcome, seen) = run_window(
        JSON_VALUE,
        true,
        WindowPlan {
            typed: Some("edited".to_string()),
            button: Some("Save"),
            ..WindowPlan::default()
        },
    );
    if seen.editor_present {
        say("PASS: an editable value opens in a text editor");
    } else {
        failures.push("an editable value did not open in a text editor".to_string());
    }
    if seen.buttons_seen == vec!["Save".to_string(), "Cancel".to_string()] {
        say("PASS: an editable value offers Save and Cancel");
    } else {
        failures.push(format!(
            "an editable value offered buttons {:?}",
            seen.buttons_seen
        ));
    }
    if outcome.as_deref() == Some("edited") {
        say("PASS: Save returns the edited text");
    } else {
        failures.push(format!("Save returned {outcome:?}, expected \"edited\""));
    }

    // 3. Cancel discards the edit.
    let (outcome, _) = run_window(
        JSON_VALUE,
        true,
        WindowPlan {
            typed: Some("edited".to_string()),
            button: Some("Cancel"),
            ..WindowPlan::default()
        },
    );
    if outcome.is_none() {
        say("PASS: Cancel discards the edit");
    } else {
        failures.push(format!("Cancel returned {outcome:?}"));
    }

    // 4. Saving unchanged text stages nothing.
    let (outcome, _) = run_window(
        JSON_VALUE,
        true,
        WindowPlan {
            button: Some("Save"),
            ..WindowPlan::default()
        },
    );
    if outcome.is_none() {
        say("PASS: saving unchanged text stages nothing");
    } else {
        failures.push(format!("saving unchanged text returned {outcome:?}"));
    }

    // 5. Format shows an indented copy and clearing it restores the exact
    //    bytes that were being edited.
    let typed = r#"{"b":2,"a":1}"#;
    let (_, seen) = run_window(
        JSON_VALUE,
        true,
        WindowPlan {
            typed: Some(typed.to_string()),
            format: true,
            unformat: true,
            button: Some("Cancel"),
            ..WindowPlan::default()
        },
    );
    let Some(expected_formatted) = value_viewer::format_json(typed) else {
        eprintln!("the sample value is not valid JSON, so this check proves nothing");
        std::process::exit(1);
    };
    if seen.text_while_formatted == expected_formatted {
        say("PASS: Format shows the indented copy");
    } else {
        failures.push(format!(
            "Format showed {:?}, expected {expected_formatted:?}",
            seen.text_while_formatted
        ));
    }
    if seen.text_after_unformat == typed {
        say("PASS: clearing Format restores the edited text byte for byte");
    } else {
        failures.push(format!(
            "clearing Format left {:?}, expected {typed:?}",
            seen.text_after_unformat
        ));
    }
    if !seen.editor_active_while_formatted {
        say("PASS: the editor is inactive while the formatted view is showing");
    } else {
        failures.push("the formatted view left the editor editable".to_string());
    }

    // 6. Saving while Format is on writes the raw value, not the indented one.
    let (outcome, _) = run_window(
        JSON_VALUE,
        true,
        WindowPlan {
            typed: Some(typed.to_string()),
            format: true,
            button: Some("Save"),
            ..WindowPlan::default()
        },
    );
    if outcome.as_deref() == Some(typed) {
        say("PASS: saving with Format on writes the raw value, not the indented view");
    } else {
        failures.push(format!(
            "saving with Format on returned {outcome:?}, expected {typed:?}"
        ));
    }

    // 7. Plain text has nothing to format, so the checkbox is dead.
    let (_, seen) = run_window(
        "just some text",
        true,
        WindowPlan {
            button: Some("Cancel"),
            ..WindowPlan::default()
        },
    );
    if !seen.format_active {
        say("PASS: Format is disabled for a value that is neither JSON nor XML");
    } else {
        failures.push("Format stayed enabled for plain text".to_string());
    }
    if value_viewer::detect_value_format("just some text") != ValueFormat::Plain {
        failures.push("plain text was detected as a formattable value".to_string());
    }

    if failures.is_empty() {
        say("\nCell value window verified end to end.");
    } else {
        eprintln!("\nFAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
    app::quit();
}
