#![allow(clippy::cargo, clippy::pedantic)]

// UI-path verification that the result grid does not leak navigation keys.
//
// An unconsumed navigation key does not stop at the focused widget: FLTK
// re-enters it as FL_SHORTCUT starting at `belowmouse()`, every group on that
// path broadcasts to all of its children, and `Fl_Scrollbar::handle()` acts on
// Up/Down/PageUp/PageDown/Home/End with no focus check at all. So a key the
// grid declines scrolls whichever pane the pointer happens to rest over.
//
// `Fl_Table` declines on its edges - `move_cursor()` returns 0 when the target
// cell is the current one - which is why this only ever reproduced on the first
// and last row/column. Real report: double-clicking a table in the object
// browser opens the grid while the pointer is still over the tree, so PageDown
// on the last row scrolled the tree instead.
//
// This harness rebuilds that exact geometry (a scrollable `Fl_Tree` as the
// pointer's pane, the real `ResultTableWidget` focused next to it), parks
// `belowmouse()` on the tree, pins the grid selection to an edge, and asserts
// the tree never moves. A plain `Fl_Tree` stands in for the object browser on
// purpose: everything under test here is FLTK's own routing, so the probe stays
// free of a database connection.
//
// Usage: cargo run --bin verify_grid_shortcut_leak

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_grid_shortcut_leak is macOS-only");
}

#[cfg(target_os = "macos")]
use fltk::{app, prelude::*, tree::Tree, window::Window};
#[cfg(target_os = "macos")]
use space_query::db::{ColumnInfo, QueryResult};
#[cfg(target_os = "macos")]
use space_query::ui::ResultTableWidget;
#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::os::raw::c_int;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
type CGEventRef = *mut c_void;
#[cfg(target_os = "macos")]
type CGEventSourceRef = *mut c_void;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
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

// macOS virtual key codes for the keys `Fl_Scrollbar` acts on without checking
// focus. Left/Right reach a horizontal scrollbar the same way.
#[cfg(target_os = "macos")]
const KEYS: &[(&str, u16)] = &[
    ("Home", 115),
    ("PageUp", 116),
    ("End", 119),
    ("PageDown", 121),
    ("Left", 123),
    ("Right", 124),
    ("Down", 125),
    ("Up", 126),
];

#[cfg(target_os = "macos")]
const GRID_ROWS: usize = 20;
#[cfg(target_os = "macos")]
const GRID_COLS: usize = 4;

#[cfg(target_os = "macos")]
fn sample_result() -> QueryResult {
    let columns = (0..GRID_COLS)
        .map(|idx| ColumnInfo {
            name: format!("COL{}", idx + 1),
            data_type: "VARCHAR2".into(),
            kind: space_query::db::SqlValueKind::String,
        })
        .collect();
    let rows = (0..GRID_ROWS)
        .map(|row| {
            (0..GRID_COLS)
                .map(|col| format!("r{row:02}c{col:02}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    QueryResult {
        sql: "SELECT * FROM shortcut_leak_probe".into(),
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
fn pump_events() {
    app::wait_for(0.05).ok();
}

#[cfg(target_os = "macos")]
fn send_key(code: u16) -> Result<(), String> {
    for down in [true, false] {
        // SAFETY: a null source requests CoreGraphics' default event source and
        // `code` is passed through as the documented virtual-key field.
        let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, down) };
        if event.is_null() {
            return Err(format!(
                "CGEventCreateKeyboardEvent returned null for {code}"
            ));
        }
        // SAFETY: `event` came from the create call above and was checked for
        // null. Posting borrows it; the single owned reference is released once.
        unsafe {
            CGEventPostToPid(std::process::id() as c_int, event);
            CFRelease(event as *const c_void);
        }
        pump_events();
    }
    Ok(())
}

/// Pin the grid selection to a corner so every key under test is a `move_cursor()`
/// no-op in at least one axis - the state the bug needed.
#[cfg(target_os = "macos")]
fn park_selection_at_corner(table: &mut fltk::table::Table, last_row: bool) {
    let row = if last_row { GRID_ROWS as i32 - 1 } else { 0 };
    let col = if last_row { GRID_COLS as i32 - 1 } else { 0 };
    table.set_selection(row, col, row, col);
    let _ = table.take_focus();
    app::set_focus(table);
    pump_events();
}

#[cfg(target_os = "macos")]
fn main() {
    let app_handle = app::App::default();

    let mut win = Window::new(200, 160, 900, 500, "verify_grid_shortcut_leak");

    // The pointer's pane. 60 items in a 460px column guarantees a scrollbar that
    // can actually move, so a leak is observable rather than silently clamped.
    let mut tree = Tree::new(10, 10, 300, 480, None);
    tree.set_root_label("SCHEMA");
    for idx in 0..60 {
        tree.add(&format!("SCHEMA/TABLE_{idx:02}"));
    }

    let mut grid = ResultTableWidget::with_size(320, 10, 570, 480);
    win.end();
    win.show();

    grid.display_result(&sample_result());
    pump_events();

    let mut table = grid.get_widget();

    // SAFETY: `AXIsProcessTrusted` takes no arguments, returns a plain boolean,
    // and is provided by the linked ApplicationServices framework.
    let trusted = unsafe { AXIsProcessTrusted() };
    println!("AXIsProcessTrusted = {trusted}");

    // Scroll the tree off its top edge: a scrollbar already at 0 cannot move
    // further up, which would hide a PageUp/Home leak behind a clamp.
    tree.set_vposition(40);
    pump_events();
    let baseline = tree.vposition();
    println!("tree vposition baseline = {baseline}");
    if baseline == 0 {
        println!("WARNING: tree did not scroll; a leak on PageUp/Home would be invisible");
    }

    // This is what made the real report path-dependent: the double-click that
    // opened the grid left the pointer over the tree, so the FL_SHORTCUT phase
    // starts there.
    app::set_belowmouse(&tree);
    pump_events();

    let mut failures = Vec::new();
    let mut delivered = false;

    for last_row in [false, true] {
        let corner = if last_row {
            "last row/col"
        } else {
            "first row/col"
        };
        println!("\n=== grid parked at {corner} ===");

        for (name, code) in KEYS {
            park_selection_at_corner(&mut table, last_row);
            app::set_belowmouse(&tree);
            let before = tree.vposition();
            let selection_before = table.get_selection();

            if let Err(err) = send_key(*code) {
                failures.push(format!("{name}: {err}"));
                continue;
            }

            let after = tree.vposition();
            let selection_after = table.get_selection();
            if selection_after != selection_before {
                delivered = true;
            }
            println!(
                "{name:<9} tree vposition {before} -> {after}   grid selection {selection_before:?} -> {selection_after:?}"
            );
            if before != after {
                delivered = true;
                failures.push(format!(
                    "{name} at {corner}: tree scrolled {before} -> {after} (key leaked to the FL_SHORTCUT broadcast)"
                ));
            }
        }
    }

    win.hide();
    app::wait_for(0.0).ok();
    let _ = app_handle;

    println!();
    // Without accessibility trust macOS drops every synthetic keystroke, so the
    // tree sitting still proves nothing - no key ever reached the grid either.
    // Reporting that as a pass would make an untrusted terminal look like a
    // verified fix, which is the one outcome worse than a failure.
    if !trusted && !delivered {
        println!(
            "SKIPPED: this process is not trusted for accessibility, so macOS discarded every\n\
             synthetic keystroke. The tree never moved, but nothing was verified.\n\
             Grant Accessibility to the terminal (System Settings > Privacy & Security >\n\
             Accessibility) and re-run from a foreground session to exercise the real path."
        );
        return;
    }

    if failures.is_empty() {
        println!("ALL SHORTCUT-LEAK CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}
