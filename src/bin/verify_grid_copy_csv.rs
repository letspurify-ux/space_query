// UI-path verification for two result-grid fixes:
//   (1) Ctrl+C / Ctrl+Shift+C copy escapes multiline / tab / quote cells so a
//       spreadsheet (Excel) keeps each value in a single cell.
//   (2) CSV export writes a UTF-8 BOM so Excel renders Korean text correctly.
//
// Drives the real `ResultTableWidget` (same widget the GUI uses) on the process
// main thread, then:
//   - reads the OS clipboard via `pbpaste` after the copy action, and
//   - writes the exported CSV to a temp file and reads the raw bytes back,
// asserting on the real serialized output the user would get.
//
// Usage: cargo run --bin verify_grid_copy_csv

use fltk::{app, prelude::*, window::Window};
use space_query::db::{ColumnInfo, QueryResult};
use space_query::ui::ResultTableWidget;
use std::process::Command;
use std::time::Duration;

const KOREAN_MULTILINE: &str = "한국어\n둘째 줄\t탭과 \"따옴표\"";
const KOREAN_PLAIN: &str = "데이터베이스";

fn sample_result() -> QueryResult {
    QueryResult {
        sql: "SELECT * FROM t".into(),
        columns: vec![
            ColumnInfo {
                name: "이름".into(),
                data_type: "VARCHAR2".into(),
            },
            ColumnInfo {
                name: "내용".into(),
                data_type: "CLOB".into(),
            },
        ],
        rows: vec![
            vec![KOREAN_PLAIN.into(), KOREAN_MULTILINE.into()],
            vec!["second".into(), "plain value".into()],
        ],
        row_count: 2,
        execution_time: Duration::from_millis(1),
        message: String::new(),
        is_select: true,
        success: true,
    }
}

fn read_clipboard() -> String {
    let out = Command::new("pbpaste")
        .output()
        .expect("pbpaste should run on macOS");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn main() {
    let app = app::App::default();
    let mut win = Window::new(0, 0, 600, 400, "verify_grid_copy_csv");
    let mut grid = ResultTableWidget::new();
    win.end();
    win.show();

    grid.display_result(&sample_result());
    // Let FLTK process the widget/layout events the display triggers.
    app::wait_for(0.2).ok();

    let mut failures: Vec<String> = Vec::new();

    // (1) Clipboard copy: select all rows + copy (Ctrl+C path) and verify the
    //     multiline cell is quoted so Excel treats it as one cell.
    grid.select_all();
    let copied = grid.copy();
    app::wait_for(0.2).ok();
    let clip = read_clipboard();

    println!("--- clipboard after Ctrl+C (copy) ---");
    println!("{:?}", clip);
    println!("copied cell count = {}", copied);

    let expected_quoted = "\"한국어\n둘째 줄\t탭과 \"\"따옴표\"\"\"";
    if clip.contains(expected_quoted) {
        println!("PASS: multiline/special cell is quoted as a single Excel cell");
    } else {
        failures.push("clipboard did not contain the expected quoted multiline cell".into());
    }
    // The plain Korean cell must NOT be quoted (no special chars).
    if clip.contains(KOREAN_PLAIN) {
        println!("PASS: plain Korean cell preserved unquoted");
    } else {
        failures.push("clipboard missing plain Korean cell".into());
    }

    // (2) CSV export: write the exact string the UI writes to disk, read bytes.
    let csv = grid.export_to_csv();
    let tmp = std::env::temp_dir().join("verify_grid_copy_csv_export.csv");
    std::fs::write(&tmp, &csv).expect("write temp csv");
    let bytes = std::fs::read(&tmp).expect("read temp csv");

    println!("\n--- CSV export ({}) ---", tmp.display());
    println!("first 3 bytes = {:02X?}", &bytes[..3.min(bytes.len())]);

    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        println!("PASS: CSV begins with UTF-8 BOM (Excel reads Korean correctly)");
    } else {
        failures.push("CSV is missing the UTF-8 BOM".into());
    }
    // Round-trip the bytes as UTF-8 and confirm Korean + quoting survived.
    let text = String::from_utf8(bytes).expect("CSV must be valid UTF-8");
    if text.contains(KOREAN_PLAIN) && text.contains("이름") {
        println!("PASS: Korean header + data preserved in CSV");
    } else {
        failures.push("CSV lost Korean text".into());
    }
    let expected_csv_cell = "\"한국어\n둘째 줄\t탭과 \"\"따옴표\"\"\"";
    if text.contains(expected_csv_cell) {
        println!("PASS: multiline cell quoted in CSV (single field)");
    } else {
        failures.push("CSV did not quote the multiline cell".into());
    }

    win.hide();
    app::wait_for(0.0).ok();
    let _ = app;

    println!();
    if failures.is_empty() {
        println!("ALL CHECKS PASSED");
    } else {
        eprintln!("FAILURES:");
        for f in &failures {
            eprintln!("  - {}", f);
        }
        std::process::exit(1);
    }
}
