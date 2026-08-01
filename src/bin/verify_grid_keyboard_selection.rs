#![allow(clippy::cargo, clippy::pedantic)]

// UI-path verification for result-grid Ctrl/Cmd + Arrow selection handling.
//
// Drives the real `ResultTableWidget` in an FLTK window, clicks an actual cell
// via CoreGraphics events, then sends Ctrl/Cmd (+Shift) arrow keystrokes and
// checks the resulting `Table::get_selection()` values.
//
// Usage: cargo run --bin verify_grid_keyboard_selection

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_grid_keyboard_selection is macOS-only");
}

#[cfg(target_os = "macos")]
use fltk::{app, prelude::*, table::TableContext, window::Window};
#[cfg(target_os = "macos")]
use space_query::db::{ColumnInfo, QueryResult};
#[cfg(target_os = "macos")]
use space_query::ui::ResultTableWidget;
#[cfg(target_os = "macos")]
use space_query::utils::arithmetic::safe_div;
#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::os::raw::c_int;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct KeyMode {
    name: &'static str,
    flags: u64,
}

#[cfg(target_os = "macos")]
const SHIFT_FLAG: u64 = 0x0002_0000;
#[cfg(target_os = "macos")]
const CONTROL_FLAG: u64 = 0x0004_0000;
#[cfg(target_os = "macos")]
const COMMAND_FLAG: u64 = 0x0010_0000;

#[cfg(target_os = "macos")]
const CONTROL_MODE: KeyMode = KeyMode {
    name: "Control",
    flags: CONTROL_FLAG,
};
#[cfg(target_os = "macos")]
const COMMAND_MODE: KeyMode = KeyMode {
    name: "Command",
    flags: COMMAND_FLAG,
};

#[cfg(target_os = "macos")]
const LEFT_MOUSE_DOWN: u32 = 1;
#[cfg(target_os = "macos")]
const LEFT_MOUSE_UP: u32 = 2;
#[cfg(target_os = "macos")]
const KEY_DOWN: bool = true;
#[cfg(target_os = "macos")]
const KEY_UP: bool = false;
#[cfg(target_os = "macos")]
const LEFT_MOUSE_BUTTON: u32 = 0;
#[cfg(target_os = "macos")]
const HID_EVENT_TAP: u32 = 0;

#[cfg(target_os = "macos")]
type CGEventRef = *mut c_void;
#[cfg(target_os = "macos")]
type CGEventSourceRef = *mut c_void;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventPostToPid(pid: c_int, event: CGEventRef);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
fn sample_result() -> QueryResult {
    let columns = (0..4)
        .map(|idx| ColumnInfo {
            name: format!("COL{}", idx + 1),
            data_type: "VARCHAR2".into(),
        })
        .collect();
    let rows = (0..20)
        .map(|row| {
            (0..4)
                .map(|col| format!("r{row:02}c{col:02}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    QueryResult {
        sql: "SELECT * FROM keyboard_selection_probe".into(),
        columns,
        row_count: rows.len(),
        rows,
        execution_time: Duration::from_millis(1),
        message: String::new(),
        is_select: true,
        success: true,
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum PostTarget {
    Pid,
    Hid,
}

#[cfg(target_os = "macos")]
impl PostTarget {
    fn label(self) -> &'static str {
        match self {
            PostTarget::Pid => "pid",
            PostTarget::Hid => "hid",
        }
    }
}

#[cfg(target_os = "macos")]
fn post_event(event: CGEventRef, target: PostTarget) -> Result<(), String> {
    if event.is_null() {
        return Err("CGEventCreate* returned null".into());
    }
    // SAFETY: `event` was returned by a CoreGraphics create function and was
    // checked for null. Posting borrows it, after which this function releases
    // its single owned CoreFoundation reference exactly once.
    unsafe {
        match target {
            PostTarget::Pid => CGEventPostToPid(std::process::id() as c_int, event),
            PostTarget::Hid => CGEventPost(HID_EVENT_TAP, event),
        }
        CFRelease(event as *const c_void);
    }
    pump_events();
    Ok(())
}

#[cfg(target_os = "macos")]
fn pump_events() {
    app::wait_for(0.15).ok();
}

#[cfg(target_os = "macos")]
fn click_at(x: i32, y: i32, target: PostTarget) -> Result<(), String> {
    let point = CGPoint {
        x: x as f64,
        y: y as f64,
    };
    // SAFETY: CoreGraphics accepts a null event source to use the default
    // source; the remaining enum values and point are initialized constants.
    let down = unsafe {
        CGEventCreateMouseEvent(
            std::ptr::null_mut(),
            LEFT_MOUSE_DOWN,
            point,
            LEFT_MOUSE_BUTTON,
        )
    };
    post_event(down, target)?;
    // SAFETY: Same preconditions as the mouse-down event above, with the
    // matching mouse-up event type.
    let up = unsafe {
        CGEventCreateMouseEvent(
            std::ptr::null_mut(),
            LEFT_MOUSE_UP,
            point,
            LEFT_MOUSE_BUTTON,
        )
    };
    post_event(up, target)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn key_code(code: u16, flags: u64) -> Result<(), String> {
    // SAFETY: A null source requests CoreGraphics' default event source, and
    // `code` is passed through as the documented virtual-key field.
    let down = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, KEY_DOWN) };
    if down.is_null() {
        return Err("CGEventCreateKeyboardEvent returned null for key down".into());
    }
    // SAFETY: `down` was checked for null and remains owned until `post_event`.
    unsafe {
        CGEventSetFlags(down, flags);
    }
    post_event(down, PostTarget::Pid)?;
    // SAFETY: Same preconditions as the key-down event above, with the
    // matching key-up state.
    let up = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, KEY_UP) };
    if up.is_null() {
        return Err("CGEventCreateKeyboardEvent returned null for key up".into());
    }
    // SAFETY: `up` was checked for null and remains owned until `post_event`.
    unsafe {
        CGEventSetFlags(up, flags);
    }
    post_event(up, PostTarget::Pid)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn click_cell(
    win: &Window,
    table: &mut fltk::table::Table,
    row: i32,
    col: i32,
) -> Result<(), String> {
    table.set_row_position(0);
    table.set_col_position(0);
    table.redraw();
    pump_events();

    let Some((cell_x, cell_y, cell_w, cell_h)) = table.find_cell(TableContext::Cell, row, col)
    else {
        return Err(format!("find_cell failed for row {row}, col {col}"));
    };

    let direct = (cell_x + safe_div(cell_w, 2), cell_y + safe_div(cell_h, 2));
    let window_relative = (win.x() + direct.0, win.y() + direct.1);
    let window_with_titlebar = (window_relative.0, window_relative.1 + 28);
    let (_, screen_y, _, screen_h) = app::screen_xywh(0);
    let invert_y = |point: (i32, i32)| (point.0, screen_y + screen_h - point.1);
    let mut candidates = if direct.0 < win.x() || direct.1 < win.y() {
        vec![
            ("window-relative", window_relative),
            ("window-titlebar-adjusted", window_with_titlebar),
            ("window-relative-inverted-y", invert_y(window_relative)),
            ("window-titlebar-inverted-y", invert_y(window_with_titlebar)),
            ("direct", direct),
        ]
    } else {
        vec![
            ("direct", direct),
            ("window-relative", window_relative),
            ("window-titlebar-adjusted", window_with_titlebar),
            ("window-relative-inverted-y", invert_y(window_relative)),
            ("window-titlebar-inverted-y", invert_y(window_with_titlebar)),
        ]
    };
    candidates.dedup_by_key(|(_, point)| *point);

    for target in [PostTarget::Pid, PostTarget::Hid] {
        for (label, (x, y)) in &candidates {
            table.unset_selection();
            table.redraw();
            pump_events();
            click_at(*x, *y, target)?;
            click_at(*x, *y, target)?;
            let selection = table.get_selection();
            println!(
                "click {target_label:>3} candidate {label:>27} at ({x}, {y}) -> selection {:?}",
                selection,
                target_label = target.label(),
            );
            if selection == (row, col, row, col) {
                let _ = table.take_focus();
                pump_events();
                return Ok(());
            }
        }
    }

    Err(format!(
        "could not click target cell ({row}, {col}); final selection {:?}",
        table.get_selection()
    ))
}

#[cfg(target_os = "macos")]
fn expect_selection(
    failures: &mut Vec<String>,
    label: &str,
    actual: (i32, i32, i32, i32),
    expected: (i32, i32, i32, i32),
) {
    println!("{label:<34} actual {actual:?}, expected {expected:?}");
    if actual != expected {
        failures.push(format!("{label}: got {actual:?}, expected {expected:?}"));
    }
}

#[cfg(target_os = "macos")]
fn run_mode_scenarios(mode: KeyMode, win: &Window, table: &mut fltk::table::Table) -> Vec<String> {
    println!("\n=== {} arrow scenarios ===", mode.name);
    let mut failures = Vec::new();

    if let Err(err) = click_cell(win, table, 5, 1) {
        failures.push(format!("{} click setup failed: {err}", mode.name));
        return failures;
    }

    if let Err(err) = key_code(125, mode.flags | SHIFT_FLAG) {
        failures.push(format!("{}+Shift+Down failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Shift+Down", mode.name),
        table.get_selection(),
        (5, 1, 19, 1),
    );

    if let Err(err) = key_code(126, mode.flags | SHIFT_FLAG) {
        failures.push(format!("{}+Shift+Up failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Shift+Up after Down", mode.name),
        table.get_selection(),
        (5, 1, 0, 1),
    );

    if let Err(err) = click_cell(win, table, 5, 1) {
        failures.push(format!("{} second click setup failed: {err}", mode.name));
        return failures;
    }

    if let Err(err) = key_code(126, mode.flags | SHIFT_FLAG) {
        failures.push(format!("{}+Shift+Up failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Shift+Up", mode.name),
        table.get_selection(),
        (5, 1, 0, 1),
    );

    if let Err(err) = key_code(125, mode.flags | SHIFT_FLAG) {
        failures.push(format!("{}+Shift+Down failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Shift+Down after Up", mode.name),
        table.get_selection(),
        (5, 1, 19, 1),
    );

    if let Err(err) = click_cell(win, table, 5, 1) {
        failures.push(format!("{} third click setup failed: {err}", mode.name));
        return failures;
    }

    if let Err(err) = key_code(125, mode.flags) {
        failures.push(format!("{}+Down failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Down", mode.name),
        table.get_selection(),
        (19, 1, 19, 1),
    );

    if let Err(err) = key_code(126, mode.flags) {
        failures.push(format!("{}+Up failed to send: {err}", mode.name));
        return failures;
    }
    expect_selection(
        &mut failures,
        &format!("{}+Up after Down", mode.name),
        table.get_selection(),
        (0, 1, 0, 1),
    );

    failures
}

#[cfg(target_os = "macos")]
fn main() {
    let app = app::App::default();
    // SAFETY: `AXIsProcessTrusted` takes no arguments, returns a plain boolean,
    // and is provided by the linked ApplicationServices framework.
    println!("AXIsProcessTrusted = {}", unsafe { AXIsProcessTrusted() });
    let mut win = Window::new(240, 180, 720, 460, "verify_grid_keyboard_selection");
    let mut grid = ResultTableWidget::with_size(10, 10, 700, 440);
    win.end();
    win.show();

    grid.display_result(&sample_result());
    pump_events();

    let mut table = grid.get_widget();
    let _ = table.take_focus();
    pump_events();

    let mut failures = Vec::new();
    failures.extend(run_mode_scenarios(CONTROL_MODE, &win, &mut table));
    if !failures.is_empty() {
        println!(
            "\nControl verification had failures; running Command fallback to separate app handler behavior from macOS Control-arrow shortcuts."
        );
        failures.extend(run_mode_scenarios(COMMAND_MODE, &win, &mut table));
    }

    win.hide();
    app::wait_for(0.0).ok();
    let _ = app;

    println!();
    if failures.is_empty() {
        println!("ALL KEYBOARD GUI CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}
