#![allow(clippy::cargo, clippy::pedantic)]

// Format-validity verification for result export.
//
// The unit tests in `src/ui/result_export.rs` pin the exact bytes each format
// produces, but exact-string assertions only prove the output matches what the
// author expected — not that a JSON parser, an XML parser, or a spreadsheet
// would accept it. This probe closes that gap: it renders one deliberately
// hostile grid in every format, hands the bytes to real parsers, and compares
// every cell that comes back to the value that was written.
//
// Hostile means every character each format has to escape, in the values *and*
// in the column names: commas, tabs, quotes, embedded newlines, a bare carriage
// return, `|`, `\`, `&`, `<`, `]]>`, a C0 control character, non-ASCII text,
// SQL NULL, an empty string, a zero-padded number, and column names that are
// blank, duplicated, punctuated, or start with a digit.
//
// Validators:
//   JSON      serde_json          (parse + numeric-aware cell round-trip)
//   XML       xmllint --noout     (well-formedness) and Python ElementTree (cells)
//   HTML      Python html.parser  (cells)
//   CSV/TSV   Python csv module   (cells, via the excel dialects)
//   Markdown  cell-count and unescape round-trip
//
// Each file is written the way the app writes it, byte-order mark included, so
// what the parsers see is what a user would open.
//
// macOS ships a 2006 HTML Tidy that predates HTML5 and misreads UTF-8, so it is
// deliberately not used here: it reports our `<!DOCTYPE html>` and `<meta
// charset>` as errors and every Korean character as an invalid code.
//
// `SQL Inserts` is not checked here: it is rendered by `grid_sql_export` and
// verified against real servers by `verify_grid_sql_export_live`.
//
// Usage: cargo run --bin verify_result_export

use space_query::db::SqlValueKind;
use space_query::ui::result_export::{render, ExportFormat, ExportGrid};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const NULL_TEXT: &str = "NULL";

/// Column names that are hostile to at least one format apiece.
fn columns() -> Vec<(&'static str, SqlValueKind)> {
    vec![
        ("ID", SqlValueKind::Number),
        // Space and parentheses are illegal in an XML element name.
        ("FULL NAME", SqlValueKind::String),
        ("COUNT(*)", SqlValueKind::Number),
        // A duplicate name: legal in SQL, and every format must still survive it.
        ("ID", SqlValueKind::Number),
        // Starts with a digit: illegal to start an XML name.
        ("2024_TOTAL", SqlValueKind::Number),
        // `SET HEADING OFF` blanks every name.
        ("", SqlValueKind::Unknown),
        ("NOTE", SqlValueKind::String),
    ]
}

/// One row per hostile trait, so a format that mishandles any of them fails.
fn rows() -> Vec<Vec<String>> {
    vec![
        vec![
            "1".into(),
            "comma, and \"quotes\"".into(),
            "00123".into(),
            "1".into(),
            "-12.5".into(),
            "tab\there".into(),
            "line1\nline2".into(),
        ],
        vec![
            "2".into(),
            "pipe | and backslash \\".into(),
            "42".into(),
            "2".into(),
            "1.2E+10".into(),
            "<tag> & \"amp\" ]]>".into(),
            "한국어 텍스트".into(),
        ],
        vec![
            "3".into(),
            NULL_TEXT.into(),
            "".into(),
            "3".into(),
            ".5".into(),
            "control\u{1}char".into(),
            "carriage\rreturn".into(),
        ],
    ]
}

fn grid() -> ExportGrid {
    let columns = columns();
    ExportGrid {
        columns: columns
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect(),
        column_kinds: columns.iter().map(|(_, kind)| *kind).collect(),
        rows: rows(),
        null_text: NULL_TEXT.to_string(),
    }
}

/// Whether a cell reads as SQL NULL to the grid, which is what the structured
/// formats render as "no value".
fn is_null_cell(row: usize, col: usize) -> bool {
    let value = &rows()[row][col];
    value.is_empty() || value == NULL_TEXT
}

/// What a value looks like after the markup formats replace the characters XML
/// and HTML cannot carry. Mirrors `escape_markup` in `result_export.rs`.
fn markup_expected(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ch,
            _ if (ch as u32) < 0x20 || ch == '\u{FFFE}' || ch == '\u{FFFF}' => '\u{FFFD}',
            _ => ch,
        })
        .collect()
}

fn main() {
    let mut failures: Vec<String> = Vec::new();
    let grid = grid();
    let dir = std::env::temp_dir().join("verify_result_export");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {err}", dir.display());
        std::process::exit(1);
    }

    for format in ExportFormat::ALL {
        if format == ExportFormat::SqlInserts {
            continue;
        }
        // What the app writes to disk: the rendered text behind the format's
        // file byte-order mark, so the parsers see real file bytes.
        let text = format!("{}{}", format.file_byte_order_mark(), render(format, &grid));
        let path = dir.join(format!("result.{}", format.extension()));
        if let Err(err) = std::fs::write(&path, text.as_bytes()) {
            failures.push(format!(
                "{}: write {}: {err}",
                format.label(),
                path.display()
            ));
            continue;
        }
        println!("\n=== {} -> {} ===", format.label(), path.display());

        let outcome = match format {
            ExportFormat::Json => check_json(&text),
            ExportFormat::Xml => check_xml(&text, &path),
            ExportFormat::Html => check_html(&text),
            ExportFormat::Csv => check_delimited(&text, ",", "excel"),
            ExportFormat::Tsv => check_delimited(&text, "\t", "excel-tab"),
            ExportFormat::Markdown => check_markdown(&text),
            ExportFormat::SqlInserts => Ok(()),
        };
        match outcome {
            Ok(()) => println!("PASS: {} is well-formed and round-trips", format.label()),
            Err(err) => failures.push(format!("{}: {err}", format.label())),
        }
    }

    if failures.is_empty() {
        println!("\nAll export formats verified with real parsers.");
    } else {
        eprintln!("\nFAILURES:");
        for failure in &failures {
            eprintln!("  - {failure}");
        }
        std::process::exit(1);
    }
}

/// Parse the JSON with `serde_json` and compare every cell back to the grid.
fn check_json(text: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("not valid JSON: {err}"))?;
    let array = value
        .as_array()
        .ok_or_else(|| "top level is not an array".to_string())?;
    let source = rows();
    if array.len() != source.len() {
        return Err(format!(
            "expected {} objects, parsed {}",
            source.len(),
            array.len()
        ));
    }

    let names = columns();
    for (row_index, entry) in array.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| format!("row {row_index} is not an object"))?;
        for (col_index, (name, _)) in names.iter().enumerate() {
            // A duplicate key keeps only the last occurrence, so a repeated
            // column can only be checked against the value that wrote it last.
            if names.iter().rposition(|(other, _)| other == name) != Some(col_index) {
                continue;
            }
            let parsed = object
                .get(*name)
                .ok_or_else(|| format!("row {row_index} is missing key {name:?}"))?;
            let expected = &source[row_index][col_index];
            if is_null_cell(row_index, col_index) {
                if !parsed.is_null() {
                    return Err(format!(
                        "row {row_index} key {name:?}: NULL became {parsed}"
                    ));
                }
                continue;
            }
            let matched = match parsed {
                serde_json::Value::String(actual) => actual == expected,
                // An unquoted number is compared numerically: `1.2E+10` is a
                // legal JSON number that every parser re-renders its own way.
                serde_json::Value::Number(actual) => actual
                    .as_f64()
                    .zip(expected.parse::<f64>().ok())
                    .is_some_and(|(actual, expected)| actual == expected),
                _ => false,
            };
            if !matched {
                return Err(format!(
                    "row {row_index} key {name:?}: {expected:?} came back as {parsed}"
                ));
            }
        }
    }
    println!("  serde_json parsed {} objects", array.len());
    Ok(())
}

/// `xmllint` for well-formedness, then ElementTree for the cell values.
fn check_xml(text: &str, path: &Path) -> Result<(), String> {
    let output = Command::new("xmllint")
        .arg("--noout")
        .arg(path)
        .output()
        .map_err(|err| format!("could not run xmllint: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "xmllint rejected the output ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    println!("  xmllint reported no errors");

    const SCRIPT: &str = "import json, sys\n\
         import xml.etree.ElementTree as ET\n\
         root = ET.fromstring(sys.stdin.read())\n\
         json.dump([[child.text or '' for child in row] for row in root], sys.stdout)\n";
    let parsed = run_python_rows(SCRIPT, &[], text)?;
    compare_body("ElementTree", &parsed, markup_expected)
}

/// Parse the HTML with Python's `html.parser` and read the table back out.
fn check_html(text: &str) -> Result<(), String> {
    const SCRIPT: &str = "import json, sys\n\
         from html.parser import HTMLParser\n\
         class TableReader(HTMLParser):\n\
         \x20   def __init__(self):\n\
         \x20       super().__init__(convert_charrefs=True)\n\
         \x20       self.rows, self.cells, self.text, self.in_cell = [], [], [], False\n\
         \x20   def handle_starttag(self, tag, attrs):\n\
         \x20       if tag == 'tr': self.cells = []\n\
         \x20       elif tag in ('td', 'th'): self.in_cell, self.text = True, []\n\
         \x20   def handle_endtag(self, tag):\n\
         \x20       if tag == 'tr': self.rows.append(self.cells)\n\
         \x20       elif tag in ('td', 'th'):\n\
         \x20           self.cells.append(''.join(self.text))\n\
         \x20           self.in_cell = False\n\
         \x20   def handle_data(self, data):\n\
         \x20       if self.in_cell: self.text.append(data)\n\
         reader = TableReader()\n\
         reader.feed(sys.stdin.read())\n\
         json.dump(reader.rows, sys.stdout)\n";
    let parsed = run_python_rows(SCRIPT, &[], text)?;
    compare_body("html.parser", &parsed, markup_expected)
}

/// Round-trip CSV/TSV through Python's `csv` module and compare every cell.
///
/// `dialect` is the Python dialect name, so the check is against a real
/// spreadsheet-compatible reader rather than a hand-written split.
fn check_delimited(text: &str, delimiter: &str, dialect: &str) -> Result<(), String> {
    // The BOM belongs in the file (Excel needs it) but is not part of the data.
    let payload = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    // The dialect and delimiter travel as argv so the script itself stays a
    // fixed string with nothing interpolated into it.
    const SCRIPT: &str = "import csv, io, json, sys\n\
         data = sys.stdin.read()\n\
         reader = csv.reader(io.StringIO(data, newline=''), dialect=sys.argv[1], delimiter=sys.argv[2])\n\
         json.dump(list(reader), sys.stdout)\n";
    let parsed = run_python_rows(SCRIPT, &[dialect, delimiter], payload)?;

    // A delimited dump is literal: NULL keeps its display text and nothing is
    // substituted, so every cell must come back byte for byte.
    let expected = full_table(str::to_string);
    compare_table("python csv", &parsed, &expected)
}

/// Every Markdown row must have the same cell count as the header, and
/// unescaping a cell must recover the value that was written.
fn check_markdown(text: &str) -> Result<(), String> {
    let lines: Vec<&str> = text.lines().collect();
    let expected_columns = columns().len();
    if lines.len() != rows().len() + 2 {
        return Err(format!(
            "expected {} lines (header + separator + rows), got {}",
            rows().len() + 2,
            lines.len()
        ));
    }
    for (index, line) in lines.iter().enumerate() {
        let cells = markdown_cells(line);
        if cells.len() != expected_columns {
            return Err(format!(
                "line {index} has {} cells, header declares {expected_columns}: {line}",
                cells.len()
            ));
        }
        if index == 1 {
            if cells.iter().any(|cell| cell != "---") {
                return Err("the separator row is not all `---`".to_string());
            }
            continue;
        }
        let expected: Vec<String> = if index == 0 {
            columns()
                .iter()
                .map(|(name, _)| markdown_expected(name))
                .collect()
        } else {
            (0..expected_columns)
                .map(|col| {
                    if is_null_cell(index - 2, col) {
                        String::new()
                    } else {
                        markdown_expected(&rows()[index - 2][col])
                    }
                })
                .collect()
        };
        let recovered: Vec<String> = cells
            .iter()
            .map(|cell| cell.replace("\\|", "|").replace("\\\\", "\\"))
            .collect();
        if recovered != expected {
            return Err(format!(
                "line {index}: read {recovered:?}, wrote {expected:?}"
            ));
        }
    }
    println!("  {} markdown rows have consistent cells", lines.len());
    Ok(())
}

/// A Markdown cell cannot hold a line break, so both kinds fold into `<br>`.
fn markdown_expected(value: &str) -> String {
    value.replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

/// Split a `| a | b |` line, respecting `\|` as an escaped pipe.
fn markdown_cells(line: &str) -> Vec<String> {
    let inner = line
        .strip_prefix("| ")
        .and_then(|rest| rest.strip_suffix(" |"))
        .unwrap_or(line);
    let mut cells = vec![String::new()];
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            if let Some(last) = cells.last_mut() {
                last.push('\\');
                last.push(ch);
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '|' => cells.push(String::new()),
            _ => {
                if let Some(last) = cells.last_mut() {
                    last.push(ch);
                }
            }
        }
    }
    cells
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// The header row plus every data row, each cell passed through `transform`.
fn full_table(transform: impl Fn(&str) -> String) -> Vec<Vec<String>> {
    let mut table = vec![columns()
        .iter()
        .map(|(name, _)| transform(name))
        .collect::<Vec<_>>()];
    for row in rows() {
        table.push(row.iter().map(|cell| transform(cell)).collect());
    }
    table
}

/// Compare a parse of a format that renders NULL as an empty cell. Only the
/// data rows are checked; the caller's parser may or may not surface a header.
fn compare_body(
    parser: &str,
    parsed: &[Vec<String>],
    transform: impl Fn(&str) -> String,
) -> Result<(), String> {
    let source = rows();
    // A header row, when the parser reports one, is dropped by matching lengths.
    let body = if parsed.len() == source.len() + 1 {
        &parsed[1..]
    } else {
        parsed
    };
    let expected: Vec<Vec<String>> = source
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.iter()
                .enumerate()
                .map(|(col_index, cell)| {
                    if is_null_cell(row_index, col_index) {
                        String::new()
                    } else {
                        transform(cell)
                    }
                })
                .collect()
        })
        .collect();
    compare_table(parser, body, &expected)
}

fn compare_table(
    parser: &str,
    parsed: &[Vec<String>],
    expected: &[Vec<String>],
) -> Result<(), String> {
    if parsed.len() != expected.len() {
        return Err(format!(
            "{parser} read {} rows, {} were written",
            parsed.len(),
            expected.len()
        ));
    }
    for (index, (got, want)) in parsed.iter().zip(expected.iter()).enumerate() {
        if got != want {
            return Err(format!("row {index}: read {got:?}, wrote {want:?}"));
        }
    }
    println!("  {parser} read {} rows with matching cells", parsed.len());
    Ok(())
}

fn run_python_rows(script: &str, args: &[&str], stdin: &str) -> Result<Vec<Vec<String>>, String> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run python3: {err}"))?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes())
            .map_err(|err| format!("could not write to python3: {err}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("python3 failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "python3 exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("could not read the python3 result: {err}"))
}
