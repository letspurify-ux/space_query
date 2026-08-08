#![allow(clippy::cargo, clippy::pedantic)]

// UI-path verification for the grid's column arrangement and value filter.
//
// The unit tests for `column_layout` and `grid_value_filter` are pure, and the
// widget tests that would cover the wiring are `#[ignore]`d because FLTK cannot
// build widgets off the main thread on macOS. This binary is the main thread:
// it drives a real `ResultTableWidget` and checks the things only the wiring can
// get wrong.
//
// Two defects this exists to keep fixed, both invisible to a pure test:
//
//   * Rows arriving in the driver's column order after a rearrangement. The
//     refusal below is what makes that impossible today — a loading grid will
//     not be rearranged — and both halves are checked here, because the two are
//     decided in different files for different reasons.
//   * A new result inheriting the old arrangement. `display_result` replaces
//     headers and rows outright, and a value filter left over from the previous
//     result still holds that result's rows — so clearing it would put another
//     query's data on screen.
//
// Usage: cargo run --bin verify_column_layout_ui

use fltk::{app, prelude::*, window::Window};
use space_query::db::{ColumnInfo, QueryResult, SqlValueKind};
use space_query::ui::column_layout::ColumnLayoutPlan;
use space_query::ui::ResultTableWidget;
use std::time::Duration;

fn columns(names: &[&str]) -> Vec<ColumnInfo> {
    names
        .iter()
        .map(|name| ColumnInfo {
            name: (*name).to_string(),
            data_type: "VARCHAR2".to_string(),
            kind: SqlValueKind::String,
        })
        .collect()
}

fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect()
}

fn result(names: &[&str], values: &[&[&str]]) -> QueryResult {
    QueryResult::new_select(
        "SELECT * FROM T",
        columns(names),
        rows(values),
        Duration::from_millis(1),
    )
}

/// Stop with a message instead of unwinding: a setup step that cannot run has
/// nothing to report, and `.expect()` is not allowed in non-test source.
fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, label: &str, ok: bool, detail: String) {
        if ok {
            println!("PASS: {label}");
        } else {
            println!("FAIL: {label} — {detail}");
            self.failures.push(format!("{label}: {detail}"));
        }
    }

    fn eq<T: std::fmt::Debug + PartialEq>(&mut self, label: &str, actual: T, expected: T) {
        let ok = actual == expected;
        self.check(label, ok, format!("got {actual:?}, expected {expected:?}"));
    }
}

/// Move the column at `from` to the front of the plan's display order.
fn move_to_front(plan: &mut ColumnLayoutPlan, from: usize) {
    let mut at = from;
    while let Some(next) = plan.move_row(at, false) {
        at = next;
    }
}

fn main() {
    let _app = app::App::default();
    let mut window = Window::new(200, 200, 720, 460, "verify_column_layout_ui");
    let mut grid = ResultTableWidget::with_size(10, 10, 700, 440);
    window.end();
    window.show();
    app::check();

    let mut report = Report {
        failures: Vec::new(),
    };

    // ---- A rearrangement moves values with their headers ----------------
    grid.start_streaming(&["A".to_string(), "B".to_string(), "C".to_string()]);
    grid.append_rows(rows(&[&["a1", "b1", "c1"]]));
    // A grid that is still loading refuses to be rearranged, which is what
    // makes the ingest order safe: every row lands before the first move.
    grid.set_lazy_fetch_session(1);
    report.check(
        "rearranging is refused while the result is still loading",
        grid.column_layout_plan().is_err(),
        "a partly loaded result accepted a rearrangement".to_string(),
    );
    grid.finish_streaming();
    app::check();

    let mut plan = grid
        .column_layout_plan()
        .unwrap_or_else(|err| fail(format!("layout plan: {err}")));
    move_to_front(&mut plan, 2);
    grid.apply_column_layout(&plan)
        .unwrap_or_else(|err| fail(format!("apply layout: {err}")));
    app::check();

    report.eq(
        "the headers follow the new order",
        grid.capture_tour_headers(),
        vec!["C".to_string(), "A".to_string(), "B".to_string()],
    );
    report.eq(
        "the row already on screen follows the headers",
        grid.capture_tour_row(0),
        Some(vec!["c1".to_string(), "a1".to_string(), "b1".to_string()]),
    );

    // ---- Rows arriving afterwards are placed the same way ----------------
    grid.append_rows(rows(&[&["a2", "b2", "c2"]]));
    app::check();
    report.eq(
        "a row appended after the rearrangement is placed too",
        grid.capture_tour_row(1),
        Some(vec!["c2".to_string(), "a2".to_string(), "b2".to_string()]),
    );

    // A short row must not slip through unplaced either.
    grid.append_rows(vec![vec!["a3".to_string()]]);
    app::check();
    report.eq(
        "a short row is padded and placed rather than left in driver order",
        grid.capture_tour_row(2),
        Some(vec![String::new(), "a3".to_string(), String::new()]),
    );

    // ---- Hiding takes the column out of what leaves the grid -------------
    let mut plan = grid
        .column_layout_plan()
        .unwrap_or_else(|err| fail(format!("layout plan: {err}")));
    plan.set_visible(1, false)
        .unwrap_or_else(|err| fail(format!("hide: {err}")));
    grid.apply_column_layout(&plan)
        .unwrap_or_else(|err| fail(format!("apply hide: {err}")));
    app::check();
    let exported = grid.export_to_csv();
    report.check(
        "a hidden column is not exported",
        !exported.contains("a1") && exported.contains("c1"),
        format!("CSV was {exported:?}"),
    );

    // ---- A new result does not inherit the old arrangement ---------------
    grid.display_result(&result(&["X", "Y", "Z"], &[&["x1", "y1", "z1"]]));
    app::check();
    report.eq(
        "a new result keeps its own column order",
        grid.capture_tour_headers(),
        vec!["X".to_string(), "Y".to_string(), "Z".to_string()],
    );
    report.eq(
        "a new result keeps its own values",
        grid.capture_tour_row(0),
        Some(vec!["x1".to_string(), "y1".to_string(), "z1".to_string()]),
    );
    let exported = grid.export_to_csv();
    report.check(
        "the previous result's hidden column does not hide one here",
        exported.contains("x1") && exported.contains("y1") && exported.contains("z1"),
        format!("CSV was {exported:?}"),
    );

    // ---- A value filter applies without a lazy fetch open ----------------
    grid.display_result(&result(
        &["ID", "GRP"],
        &[&["1", "alpha"], &["2", "beta"], &["3", "alpha"]],
    ));
    app::check();
    grid.capture_tour_select_range(0, 1, 0, 1);
    let filter = grid
        .value_filter_from_selection(false)
        .unwrap_or_else(|err| fail(format!("build filter: {err}")));
    let outcome = grid
        .apply_value_filter(filter)
        .unwrap_or_else(|err| fail(format!("apply filter: {err}")));
    report.eq("the filter keeps the matching rows", outcome.kept_rows, 2);
    report.eq("the filter reports the whole result", outcome.total_rows, 3);
    report.check(
        "the filter describes itself",
        outcome.description.contains("GRP") && outcome.description.contains("alpha"),
        format!("description was {:?}", outcome.description),
    );

    // ---- Clearing puts back this result's rows, not the previous one's ----
    report.check(
        "clearing reports it did something",
        grid.clear_value_filter(),
        String::new(),
    );
    app::check();
    report.eq(
        "clearing restores every row of the current result",
        grid.capture_tour_row_count(),
        3,
    );
    report.eq(
        "clearing restores this result's values",
        grid.capture_tour_row(0),
        Some(vec!["1".to_string(), "alpha".to_string()]),
    );

    // ---- A new result drops a live filter rather than restoring into it ---
    grid.capture_tour_select_range(0, 1, 0, 1);
    let filter = grid
        .value_filter_from_selection(false)
        .unwrap_or_else(|err| fail(format!("build filter: {err}")));
    let _ = grid.apply_value_filter(filter);
    grid.display_result(&result(&["ID", "GRP"], &[&["9", "zulu"]]));
    app::check();
    report.check(
        "a new result arrives unfiltered",
        !grid.value_filter_is_active(),
        "the previous result's filter was still active".to_string(),
    );
    report.eq(
        "the new result shows its own single row",
        grid.capture_tour_row(0),
        Some(vec!["9".to_string(), "zulu".to_string()]),
    );

    window.hide();
    app::wait_for(0.0).ok();

    println!();
    if report.failures.is_empty() {
        println!("ALL COLUMN LAYOUT UI CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for failure in &report.failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}
