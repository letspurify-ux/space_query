//! Serialize a result grid into an export format.
//!
//! Pure functions over a snapshot of the grid — no FLTK, no database — so every
//! byte that reaches a file or the clipboard is unit-testable.
//!
//! `SQL Inserts` is the one format that is not rendered here: it needs the
//! connection's dialect and the resolved base table, and that logic already
//! lives in [`crate::ui::grid_sql_export`]. [`render`] returns an empty string
//! for it so a caller that forgets to route it produces nothing rather than
//! something wrong.

use crate::db::SqlValueKind;
use crate::ui::result_table::ResultTableWidget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Xml,
    Html,
    Markdown,
    SqlInserts,
}

impl ExportFormat {
    /// Offered order in the export dialog. CSV first because it is what the
    /// keyboard shortcut used to do on its own.
    pub const ALL: [ExportFormat; 7] = [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::Xml,
        ExportFormat::Html,
        ExportFormat::Markdown,
        ExportFormat::SqlInserts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "TSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Xml => "XML",
            ExportFormat::Html => "HTML",
            ExportFormat::Markdown => "Markdown",
            ExportFormat::SqlInserts => "SQL Inserts",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::Xml => "xml",
            ExportFormat::Html => "html",
            ExportFormat::Markdown => "md",
            ExportFormat::SqlInserts => "sql",
        }
    }

    /// Single-entry filter string for the native save chooser.
    pub fn file_filter(self) -> String {
        format!("{} Files\t*.{}", self.label(), self.extension())
    }

    /// What a *file* of this format starts with, before the rendered text.
    ///
    /// Excel decides a delimited file's encoding from a UTF-8 BOM and otherwise
    /// falls back to the system locale, mangling non-ASCII text. Nothing else
    /// needs it, and the clipboard must never carry one: pasting U+FEFF into an
    /// editor inserts an invisible character nobody asked for.
    pub fn file_byte_order_mark(self) -> &'static str {
        match self {
            ExportFormat::Csv | ExportFormat::Tsv => "\u{FEFF}",
            ExportFormat::Json
            | ExportFormat::Xml
            | ExportFormat::Html
            | ExportFormat::Markdown
            | ExportFormat::SqlInserts => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportScope {
    All,
    Selection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportDestination {
    File,
    Clipboard,
}

/// The rows and columns being exported, already narrowed to the chosen scope.
#[derive(Clone, Debug, Default)]
pub struct ExportGrid {
    pub columns: Vec<String>,
    /// Per-column literal kind. Shorter than `columns` means "unknown", which
    /// only costs JSON its unquoted numbers.
    pub column_kinds: Vec<SqlValueKind>,
    pub rows: Vec<Vec<String>>,
    /// Display text the grid uses for SQL NULL.
    pub null_text: String,
}

impl ExportGrid {
    fn cell<'a>(&self, row: &'a [String], index: usize) -> &'a str {
        row.get(index).map_or("", String::as_str)
    }

    fn is_null(&self, row: &[String], index: usize) -> bool {
        ResultTableWidget::value_represents_null(self.cell(row, index), &self.null_text)
    }

    fn kind(&self, index: usize) -> SqlValueKind {
        self.column_kinds
            .get(index)
            .copied()
            .unwrap_or(SqlValueKind::Unknown)
    }
}

/// Render `grid` in `format`.
///
/// CSV and TSV are literal dumps of what the grid shows, NULL display text
/// included, because that is what a spreadsheet round-trip expects. The
/// structured formats have their own way to say "no value" and use it.
pub fn render(format: ExportFormat, grid: &ExportGrid) -> String {
    match format {
        ExportFormat::Csv => render_separated(grid, ',', escape_csv_field),
        ExportFormat::Tsv => render_separated(grid, '\t', escape_tab_separated_field),
        ExportFormat::Json => render_json(grid),
        ExportFormat::Xml => render_xml(grid),
        ExportFormat::Html => render_html(grid),
        ExportFormat::Markdown => render_markdown(grid),
        // Dialect-dependent; `grid_sql_export::build_sql_inserts` owns it.
        ExportFormat::SqlInserts => String::new(),
    }
}

/// CSV / TSV share everything but the separator and the escape rule.
///
/// The line ending follows the platform, because these two are what a
/// spreadsheet opens. The UTF-8 BOM Excel needs is not here: it belongs to a
/// file, not to the text, and [`ExportFormat::file_byte_order_mark`] adds it on
/// the way to disk.
fn render_separated(grid: &ExportGrid, separator: char, escape: fn(&str) -> String) -> String {
    let line_ending = csv_line_ending();
    let mut out = String::with_capacity(grid.rows.len() * 20 + grid.columns.len() * 16 + 4);
    for (index, column) in grid.columns.iter().enumerate() {
        if index > 0 {
            out.push(separator);
        }
        out.push_str(&escape(column));
    }
    out.push_str(line_ending);

    for row in &grid.rows {
        for index in 0..grid.columns.len() {
            if index > 0 {
                out.push(separator);
            }
            out.push_str(&escape(grid.cell(row, index)));
        }
        out.push_str(line_ending);
    }
    out
}

pub(crate) fn csv_line_ending() -> &'static str {
    if cfg!(windows) {
        "\r\n"
    } else {
        "\n"
    }
}

pub(crate) fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Escape a cell for tab-separated output so spreadsheet apps (Excel/Sheets)
/// keep multiline and tab-containing values in a single cell. Fields containing
/// a tab, newline, carriage return, or quote are wrapped in double quotes with
/// embedded quotes doubled, matching the convention those apps use when parsing
/// pasted TSV text.
pub(crate) fn escape_tab_separated_field(field: &str) -> String {
    if field.contains('\t') || field.contains('\n') || field.contains('\r') || field.contains('"') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn render_json(grid: &ExportGrid) -> String {
    if grid.rows.is_empty() {
        return "[]\n".to_string();
    }
    let mut out = String::from("[\n");
    for (row_index, row) in grid.rows.iter().enumerate() {
        out.push_str("  {\n");
        for (index, column) in grid.columns.iter().enumerate() {
            out.push_str("    ");
            out.push_str(&json_string(column));
            out.push_str(": ");
            out.push_str(&json_value(grid, row, index));
            if index + 1 < grid.columns.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  }");
        if row_index + 1 < grid.rows.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

/// A JSON scalar for one cell.
///
/// Only a column the driver typed as numeric or boolean is emitted unquoted,
/// and only when its text is already valid JSON — that keeps a zero-padded code
/// like `00123` a string and never produces invalid JSON from an unexpected
/// rendering (an Oracle `NUMBER` shown in scientific notation, say).
fn json_value(grid: &ExportGrid, row: &[String], index: usize) -> String {
    if grid.is_null(row, index) {
        return "null".to_string();
    }
    let value = grid.cell(row, index);
    let trimmed = value.trim();
    match grid.kind(index) {
        SqlValueKind::Boolean => {
            if trimmed.eq_ignore_ascii_case("true") {
                "true".to_string()
            } else if trimmed.eq_ignore_ascii_case("false") {
                "false".to_string()
            } else if is_json_number(trimmed) {
                trimmed.to_string()
            } else {
                json_string(value)
            }
        }
        SqlValueKind::Number => {
            if is_json_number(trimmed) {
                trimmed.to_string()
            } else {
                json_string(value)
            }
        }
        SqlValueKind::String
        | SqlValueKind::Temporal
        | SqlValueKind::Binary
        | SqlValueKind::Unknown => json_string(value),
    }
}

/// Whether `value` is a number literal the JSON grammar accepts verbatim.
/// Deliberately strict: leading zeros, a leading `+`, and a bare `.5` are all
/// rejected so they stay quoted strings.
fn is_json_number(value: &str) -> bool {
    let mut chars = value.chars().peekable();
    if chars.peek() == Some(&'-') {
        chars.next();
    }
    // Integer part: a lone `0`, or a non-zero digit followed by digits.
    match chars.next() {
        Some('0') => {}
        Some(digit) if digit.is_ascii_digit() => {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        }
        _ => return false,
    }
    if chars.peek() == Some(&'.') {
        chars.next();
        if !chars.peek().is_some_and(char::is_ascii_digit) {
            return false;
        }
        while chars.peek().is_some_and(char::is_ascii_digit) {
            chars.next();
        }
    }
    if matches!(chars.peek(), Some('e' | 'E')) {
        chars.next();
        if matches!(chars.peek(), Some('+' | '-')) {
            chars.next();
        }
        if !chars.peek().is_some_and(char::is_ascii_digit) {
            return false;
        }
        while chars.peek().is_some_and(char::is_ascii_digit) {
            chars.next();
        }
    }
    chars.next().is_none()
}

/// Quote and escape a JSON string. `serde_json` owns the escaping rules so
/// control characters come out as `\uXXXX` rather than raw bytes.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn render_xml(grid: &ExportGrid) -> String {
    let names: Vec<String> = grid
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| xml_element_name(column, index))
        .collect();

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<results>\n");
    for row in &grid.rows {
        out.push_str("  <row>\n");
        for (index, name) in names.iter().enumerate() {
            if grid.is_null(row, index) {
                out.push_str(&format!("    <{name}/>\n"));
            } else {
                out.push_str(&format!(
                    "    <{name}>{}</{name}>\n",
                    escape_xml_text(grid.cell(row, index))
                ));
            }
        }
        out.push_str("  </row>\n");
    }
    out.push_str("</results>\n");
    out
}

/// Turn a column name into a legal XML element name.
///
/// Result columns can be expressions (`COUNT(*)`), blank (`SET HEADING OFF`),
/// or start with a digit, none of which XML accepts, so anything unusable
/// becomes `column_<n>` and every other illegal character becomes `_`.
fn xml_element_name(column: &str, index: usize) -> String {
    let mut name = String::with_capacity(column.len());
    for ch in column.trim().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            name.push(ch);
        } else {
            name.push('_');
        }
    }
    let starts_legally = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_');
    if !starts_legally || name.to_ascii_lowercase().starts_with("xml") {
        return format!("column_{}", index + 1);
    }
    name
}

fn escape_xml_text(value: &str) -> String {
    escape_markup(value, false)
}

/// Escape a value for an XML or HTML text node.
///
/// Beyond the usual entities this drops characters the markup grammars reject
/// outright. XML 1.0's `Char` production excludes every C0 control except tab,
/// newline, and carriage return — and unlike `<`, they cannot be rescued by a
/// character reference either, so a `CHAR` column carrying one would otherwise
/// produce a document no parser accepts. They become U+FFFD, which makes the
/// substitution visible instead of silently shortening the value.
fn escape_markup(value: &str, html: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if html => out.push_str("&quot;"),
            // A literal CR is folded into LF by both the XML line-end rules and
            // the HTML5 input-stream preprocessor. A character reference is
            // resolved after that folding, so this is what keeps a value that
            // really contains CR intact.
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(ch),
            _ if (ch as u32) < 0x20 || ch == '\u{FFFE}' || ch == '\u{FFFF}' => {
                out.push('\u{FFFD}');
            }
            _ => out.push(ch),
        }
    }
    out
}

fn render_html(grid: &ExportGrid) -> String {
    let mut out = String::from(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>Query Result</title>\n\
         <style>\n\
         table { border-collapse: collapse; font-family: sans-serif; font-size: 13px; }\n\
         th, td { border: 1px solid #999; padding: 4px 8px; text-align: left; }\n\
         th { background: #eee; }\n\
         </style>\n</head>\n<body>\n<table>\n<thead>\n<tr>",
    );
    for column in &grid.columns {
        out.push_str(&format!("<th>{}</th>", escape_html_text(column)));
    }
    out.push_str("</tr>\n</thead>\n<tbody>\n");
    for row in &grid.rows {
        out.push_str("<tr>");
        for index in 0..grid.columns.len() {
            let cell = if grid.is_null(row, index) {
                String::new()
            } else {
                escape_html_text(grid.cell(row, index))
            };
            out.push_str(&format!("<td>{cell}</td>"));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n</body>\n</html>\n");
    out
}

fn escape_html_text(value: &str) -> String {
    escape_markup(value, true)
}

fn render_markdown(grid: &ExportGrid) -> String {
    let mut out = String::new();
    out.push_str("| ");
    out.push_str(
        &grid
            .columns
            .iter()
            .map(|column| escape_markdown_cell(column))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push_str(" |\n| ");
    out.push_str(&vec!["---"; grid.columns.len()].join(" | "));
    out.push_str(" |\n");

    for row in &grid.rows {
        out.push_str("| ");
        let cells: Vec<String> = (0..grid.columns.len())
            .map(|index| {
                if grid.is_null(row, index) {
                    String::new()
                } else {
                    escape_markdown_cell(grid.cell(row, index))
                }
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    out
}

/// A Markdown table cell cannot contain a raw `|` or a line break, so the pipe
/// is escaped and every line break becomes `<br>`.
fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> ExportGrid {
        ExportGrid {
            columns: vec!["ID".to_string(), "NAME".to_string()],
            column_kinds: vec![SqlValueKind::Number, SqlValueKind::String],
            rows: vec![
                vec!["1".to_string(), "alpha".to_string()],
                vec!["2".to_string(), "NULL".to_string()],
            ],
            null_text: "NULL".to_string(),
        }
    }

    #[test]
    fn csv_keeps_the_header_and_platform_line_ending() {
        let line_ending = csv_line_ending();
        assert_eq!(
            render(ExportFormat::Csv, &grid()),
            format!("ID,NAME{line_ending}1,alpha{line_ending}2,NULL{line_ending}")
        );
    }

    #[test]
    fn only_the_spreadsheet_formats_carry_a_file_byte_order_mark() {
        assert_eq!(ExportFormat::Csv.file_byte_order_mark(), "\u{FEFF}");
        assert_eq!(ExportFormat::Tsv.file_byte_order_mark(), "\u{FEFF}");
        for format in [
            ExportFormat::Json,
            ExportFormat::Xml,
            ExportFormat::Html,
            ExportFormat::Markdown,
            ExportFormat::SqlInserts,
        ] {
            assert_eq!(format.file_byte_order_mark(), "", "{}", format.label());
        }
    }

    #[test]
    fn rendered_text_never_starts_with_a_byte_order_mark() {
        // The BOM is a property of the file, not of the text: the clipboard
        // must not receive one.
        for format in ExportFormat::ALL {
            assert!(
                !render(format, &grid()).starts_with('\u{FEFF}'),
                "{} rendered a BOM into its text",
                format.label()
            );
        }
    }

    #[test]
    fn csv_quotes_separators_quotes_and_line_breaks() {
        assert_eq!(escape_csv_field("a,b"), "\"a,b\"");
        assert_eq!(escape_csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape_csv_field("line1\rline2"), "\"line1\rline2\"");
        assert_eq!(escape_csv_field("plain\ttext"), "plain\ttext");
    }

    #[test]
    fn tsv_quotes_tabs_but_not_commas() {
        assert_eq!(escape_tab_separated_field("a,b"), "a,b");
        assert_eq!(escape_tab_separated_field("a\tb"), "\"a\tb\"");
    }

    #[test]
    fn tsv_separates_with_tabs() {
        let line_ending = csv_line_ending();
        assert_eq!(
            render(ExportFormat::Tsv, &grid()),
            format!("ID\tNAME{line_ending}1\talpha{line_ending}2\tNULL{line_ending}")
        );
    }

    #[test]
    fn separated_formats_still_emit_a_header_for_an_empty_result() {
        let empty = ExportGrid {
            columns: vec!["A".to_string()],
            rows: Vec::new(),
            ..ExportGrid::default()
        };
        assert_eq!(
            render(ExportFormat::Csv, &empty),
            format!("A{}", csv_line_ending())
        );
    }

    #[test]
    fn json_emits_null_for_grid_null_text() {
        assert_eq!(
            render(ExportFormat::Json, &grid()),
            "[\n  {\n    \"ID\": 1,\n    \"NAME\": \"alpha\"\n  },\n  \
             {\n    \"ID\": 2,\n    \"NAME\": null\n  }\n]\n"
        );
    }

    #[test]
    fn json_of_an_empty_result_is_an_empty_array() {
        let empty = ExportGrid {
            columns: vec!["A".to_string()],
            ..ExportGrid::default()
        };
        assert_eq!(render(ExportFormat::Json, &empty), "[]\n");
    }

    #[test]
    fn json_quotes_a_numeric_column_whose_text_is_not_a_json_number() {
        let padded = ExportGrid {
            columns: vec!["CODE".to_string()],
            column_kinds: vec![SqlValueKind::Number],
            // A zero-padded code and Oracle's leading-dot rendering of 0.5;
            // `1.2E+10` is deliberately alongside them because exponent notation
            // *is* a JSON number and must stay unquoted.
            rows: vec![
                vec!["00123".to_string()],
                vec![".5".to_string()],
                vec!["1.2E+10".to_string()],
            ],
            null_text: "NULL".to_string(),
        };
        assert_eq!(
            render(ExportFormat::Json, &padded),
            "[\n  {\n    \"CODE\": \"00123\"\n  },\n  {\n    \"CODE\": \".5\"\n  },\n  \
             {\n    \"CODE\": 1.2E+10\n  }\n]\n"
        );
    }

    #[test]
    fn json_number_grammar_matches_the_spec() {
        for accepted in ["0", "-0", "12", "-12.5", "1e10", "2.5E-3", "1E+3"] {
            assert!(is_json_number(accepted), "{accepted} should be accepted");
        }
        for rejected in ["", "-", "007", "+1", ".5", "1.", "1e", "1e+", "0x10", "1 2"] {
            assert!(!is_json_number(rejected), "{rejected} should be rejected");
        }
    }

    #[test]
    fn json_escapes_control_characters() {
        let control = ExportGrid {
            columns: vec!["V".to_string()],
            rows: vec![vec!["a\tb\u{1}".to_string()]],
            ..ExportGrid::default()
        };
        assert!(render(ExportFormat::Json, &control).contains("\"a\\tb\\u0001\""));
    }

    #[test]
    fn xml_uses_an_empty_element_for_null_and_escapes_markup() {
        let markup = ExportGrid {
            columns: vec!["A".to_string(), "B".to_string()],
            rows: vec![vec!["x & <y>".to_string(), "NULL".to_string()]],
            null_text: "NULL".to_string(),
            ..ExportGrid::default()
        };
        assert_eq!(
            render(ExportFormat::Xml, &markup),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<results>\n  <row>\n    \
             <A>x &amp; &lt;y&gt;</A>\n    <B/>\n  </row>\n</results>\n"
        );
    }

    #[test]
    fn markup_replaces_characters_xml_cannot_carry_at_all() {
        // U+0001 has no character reference in XML 1.0; only tab, newline, and
        // carriage return survive from the C0 block.
        assert_eq!(escape_xml_text("a\u{1}b"), "a\u{FFFD}b");
        assert_eq!(escape_html_text("a\u{1}b"), "a\u{FFFD}b");
        // CR survives only as a character reference; written literally, both
        // grammars would fold it into a newline.
        assert_eq!(escape_xml_text("a\tb\nc\rd"), "a\tb\nc&#13;d");
        assert_eq!(escape_xml_text("a\u{FFFF}b"), "a\u{FFFD}b");
    }

    #[test]
    fn markup_escapes_quotes_only_for_html() {
        assert_eq!(
            escape_xml_text("say \"hi\" & <bye>"),
            "say \"hi\" &amp; &lt;bye&gt;"
        );
        assert_eq!(
            escape_html_text("say \"hi\" & <bye>"),
            "say &quot;hi&quot; &amp; &lt;bye&gt;"
        );
    }

    #[test]
    fn xml_element_names_replace_illegal_characters() {
        assert_eq!(xml_element_name("FIRST NAME", 0), "FIRST_NAME");
        assert_eq!(xml_element_name("COUNT(*)", 0), "COUNT___");
        assert_eq!(xml_element_name("SUM", 0), "SUM");
    }

    #[test]
    fn xml_element_names_fall_back_when_the_name_cannot_start_one() {
        assert_eq!(xml_element_name("", 0), "column_1");
        assert_eq!(xml_element_name("2024", 1), "column_2");
        // XML reserves any name starting with "xml", in any case.
        assert_eq!(xml_element_name("XmlPayload", 3), "column_4");
        // `_` is a legal first character, so an all-punctuation name survives
        // as underscores rather than needing the positional fallback.
        assert_eq!(xml_element_name("(*)", 2), "___");
    }

    #[test]
    fn html_escapes_markup_and_leaves_null_cells_empty() {
        let markup = ExportGrid {
            columns: vec!["A<b>".to_string()],
            rows: vec![vec!["1 & 2".to_string()], vec!["NULL".to_string()]],
            null_text: "NULL".to_string(),
            ..ExportGrid::default()
        };
        let html = render(ExportFormat::Html, &markup);
        assert!(html.contains("<th>A&lt;b&gt;</th>"));
        assert!(html.contains("<td>1 &amp; 2</td>"));
        assert!(html.contains("<td></td>"));
        assert!(html.ends_with("</html>\n"));
    }

    #[test]
    fn markdown_writes_a_pipe_table_with_a_separator_row() {
        assert_eq!(
            render(ExportFormat::Markdown, &grid()),
            "| ID | NAME |\n| --- | --- |\n| 1 | alpha |\n| 2 |  |\n"
        );
    }

    #[test]
    fn markdown_escapes_pipes_and_folds_line_breaks() {
        assert_eq!(escape_markdown_cell("a|b"), "a\\|b");
        assert_eq!(escape_markdown_cell("a\r\nb"), "a<br>b");
        assert_eq!(escape_markdown_cell("a\nb"), "a<br>b");
        assert_eq!(escape_markdown_cell("c:\\dir"), "c:\\\\dir");
    }

    #[test]
    fn sql_inserts_renders_nothing_here() {
        // The dialect-aware builder in `grid_sql_export` owns this format; a
        // caller that routes it to `render` must get nothing, not wrong SQL.
        assert!(render(ExportFormat::SqlInserts, &grid()).is_empty());
    }

    #[test]
    fn every_format_has_a_distinct_label_and_extension() {
        let mut labels: Vec<&str> = ExportFormat::ALL.iter().map(|f| f.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ExportFormat::ALL.len());

        let mut extensions: Vec<&str> = ExportFormat::ALL.iter().map(|f| f.extension()).collect();
        extensions.sort_unstable();
        extensions.dedup();
        assert_eq!(extensions.len(), ExportFormat::ALL.len());
    }

    #[test]
    fn file_filter_pairs_the_label_with_the_extension() {
        assert_eq!(ExportFormat::Json.file_filter(), "JSON Files\t*.json");
    }
}
