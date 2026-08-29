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

/// What one cell of an export holds: the text the grid shows, or SQL NULL.
///
/// The mirror of [`crate::ui::result_import::ImportCell`], and for the same
/// reason: "is this NULL?" is a question with one answer, decided once where it
/// is still knowable, not re-derived from text by every serializer. Text that
/// merely READS like a NULL — an empty string, the four letters `NULL` — is
/// `Some`, and each format then spells the two apart in its own vocabulary.
pub type ExportCell = Option<String>;

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

/// What a render or a SQL build produced: the text, and how many rows of the
/// source it covers — or the sentence saying why nothing was written.
///
/// One value, because the two are one fact. The row count used to be read off
/// the builder's INPUT ([`render_export_content`] returned
/// `selection.rows.len()` whatever came back), and every early return in
/// [`crate::ui::grid_sql_export::build_sql_inserts`] writes nothing — so an
/// empty file was announced as "N rows exported" and an empty clipboard as
/// "Copied N INSERT statements".
///
/// `Refused` is the third thing a build can be. It used to be spelled as an
/// empty string, which a caller cannot tell from an empty result, so every
/// refusal had to be asked by a separate gate that a caller could forget. The
/// gate now lives inside the builder and its answer comes back with the rest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportContent {
    /// `rows` is what the text actually covers, not what it was offered.
    Written { text: String, rows: usize },
    /// Nothing was written, and this is what to tell the user.
    Refused(String),
}

impl ExportContent {
    pub fn written(text: String, rows: usize) -> Self {
        ExportContent::Written { text, rows }
    }

    /// A build that had nothing to write and nothing to complain about.
    pub fn nothing() -> Self {
        ExportContent::Written {
            text: String::new(),
            rows: 0,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            ExportContent::Written { text, .. } => text.as_str(),
            ExportContent::Refused(_) => "",
        }
    }

    pub fn rows(&self) -> usize {
        match self {
            ExportContent::Written { rows, .. } => *rows,
            ExportContent::Refused(_) => 0,
        }
    }

    pub fn refusal(&self) -> Option<&str> {
        match self {
            ExportContent::Written { .. } => None,
            ExportContent::Refused(reason) => Some(reason.as_str()),
        }
    }

    /// Rewrite the text that was written, leaving a refusal alone.
    ///
    /// A refusal has no text to decorate: putting a byte-order mark in front of
    /// one would turn "nothing was written" into a one-character file.
    pub fn map_text(self, rewrite: impl FnOnce(String) -> String) -> Self {
        match self {
            ExportContent::Written { text, rows } => ExportContent::Written {
                text: rewrite(text),
                rows,
            },
            refused => refused,
        }
    }

    /// The text and its row count, or the refusal to report.
    pub fn into_parts(self) -> Result<(String, usize), String> {
        match self {
            ExportContent::Written { text, rows } => Ok((text, rows)),
            ExportContent::Refused(reason) => Err(reason),
        }
    }
}

/// What a grid hands back once it is ready to be exported.
///
/// Two shapes, because one format cannot be finished where the rows are read.
/// A `SQL Inserts` script is meant to be RE-RUN, so it must not name a column
/// the server computes — and only the catalog knows which those are, which is a
/// round trip. So the grid hands back the SNAPSHOT it would build from instead
/// of the text, and the round trip happens with the rows already in hand.
///
/// The point is what happens AFTER this value exists: nothing reads the grid
/// again. The export road used to re-resolve *which grid* when the catalog
/// answered, and it asked by TABLE NAME — so a re-run of the same query, a
/// click on another result tab showing the same table, or a changed selection
/// during that round trip silently redirected the file the user had already
/// named. A payload cannot be redirected: it holds exactly what the file will
/// contain.
#[derive(Clone, Debug)]
pub enum ExportPayload {
    /// Rendered text, ready for its destination.
    Data(ExportContent),
    /// The rows and columns a `SQL Inserts` build will use once the catalog has
    /// answered which columns the server computes.
    Sql(crate::ui::grid_sql_export::GridSqlSelection),
}

impl ExportPayload {
    /// The finished bytes, for a caller that only ever asks for a data format.
    ///
    /// `None` says the payload is a `SQL Inserts` snapshot, which is not text
    /// yet — a caller that gets one asked for a format it does not handle.
    pub fn data(self) -> Option<ExportContent> {
        match self {
            ExportPayload::Data(content) => Some(content),
            ExportPayload::Sql(_) => None,
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
    /// Rows aligned to `columns`. SQL NULL is [`None`]; `Some("")` is the empty
    /// string, which is a different value everywhere but Oracle.
    pub rows: Vec<Vec<ExportCell>>,
    /// Display text the grid uses for SQL NULL. Only the spreadsheet formats
    /// read it: CSV and TSV are literal dumps of what the grid shows, so a NULL
    /// has to reach the file wearing the same text the user sees.
    pub null_text: String,
}

impl ExportGrid {
    /// The cell at `index`. A column the row does not reach has no value at
    /// all, which is SQL NULL.
    fn cell<'a>(&self, row: &'a [ExportCell], index: usize) -> &'a ExportCell {
        row.get(index).unwrap_or(&None)
    }

    /// What CSV and TSV write for one cell: the text, and whether it must be
    /// quoted even where the escaping rules would not ask for it.
    ///
    /// A NULL is written as the grid's NULL display text, because a delimited
    /// file is a dump of what the grid shows. A VALUE whose text happens to BE
    /// that display text is written QUOTED — the one signal a delimited file
    /// has left to tell the two apart, and the one
    /// [`crate::ui::result_import::parse`] reads. Without it this app's own
    /// export → import turned a `VARCHAR` holding the four letters `NULL` into
    /// SQL NULL, silently, on every backend.
    ///
    /// An EMPTY value is quoted for the same reason. A row of one column
    /// holding the empty string would otherwise be an empty line, and an empty
    /// line is a blank one — which no reader takes for a record, this app's
    /// included. `""` is a record, and every reader gives back the same string
    /// for it.
    ///
    /// The signal is only free while the NULL text needs no quotes of its own.
    /// One that holds the separator, a quote, or a line break is quoted
    /// whatever this says, so both spellings look alike and the reader falls
    /// back to matching the text — see
    /// [`crate::ui::result_import::null_text_quoting_is_a_signal`], which is
    /// the same question asked from the other side.
    ///
    /// A NULL is never force-quoted: bare is what says it is one. The honest
    /// limit that leaves is a NULL text set to EMPTY in a result of one column,
    /// where a NULL really is an empty line and nothing can tell it from a
    /// blank one.
    fn display_cell<'a>(&'a self, row: &'a [ExportCell], index: usize) -> (&'a str, bool) {
        match self.cell(row, index) {
            Some(value) => (
                value.as_str(),
                value.is_empty() || value.as_str() == self.null_text,
            ),
            None => (self.null_text.as_str(), false),
        }
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
        ExportFormat::Csv => render_separated(grid, ','),
        ExportFormat::Tsv => render_separated(grid, '\t'),
        ExportFormat::Json => render_json(grid),
        ExportFormat::Xml => render_xml(grid),
        ExportFormat::Html => render_html(grid),
        ExportFormat::Markdown => render_markdown(grid),
        // Dialect-dependent; `grid_sql_export::build_sql_inserts` owns it.
        ExportFormat::SqlInserts => String::new(),
    }
}

/// Render an export in `format`, choosing between the plain serializers above
/// and the dialect-aware `SQL Inserts` builder.
///
/// The fork lives here rather than in the grid because the object browser
/// exports a table without going through a grid at all. One fork means the two
/// callers cannot disagree about which renderer a format belongs to — and in
/// particular cannot route `SqlInserts` to [`render`], which deliberately
/// answers with an empty string.
///
/// `sql_selection` is only read for `SqlInserts`; passing `None` for that format
/// yields nothing rather than wrong SQL.
///
/// The row count comes from what was WRITTEN, never from what was offered:
/// `SqlInserts` has three ways to write nothing (a repeated column name, no
/// nameable column, a value no literal can carry) and each used to be reported
/// as a full export of an empty file.
pub fn render_export_content(
    format: ExportFormat,
    grid: &ExportGrid,
    sql_selection: Option<&crate::ui::grid_sql_export::GridSqlSelection>,
) -> ExportContent {
    match format {
        ExportFormat::SqlInserts => match sql_selection {
            Some(selection) => crate::ui::grid_sql_export::build_sql_inserts(selection),
            None => ExportContent::nothing(),
        },
        format => ExportContent::written(render(format, grid), grid.rows.len()),
    }
}

/// Put `format`'s byte-order mark in front of `text` when the bytes are headed
/// for a file.
///
/// A BOM tells a spreadsheet how to decode a *file*; on the clipboard it just
/// pastes an invisible `U+FEFF` wherever it lands.
pub fn with_destination_prelude(
    format: ExportFormat,
    destination: ExportDestination,
    text: String,
) -> String {
    let prelude = match destination {
        ExportDestination::File => format.file_byte_order_mark(),
        ExportDestination::Clipboard => "",
    };
    if prelude.is_empty() {
        text
    } else {
        format!("{prelude}{text}")
    }
}

/// CSV / TSV share everything but the separator and the escape rule.
///
/// The line ending follows the platform, because these two are what a
/// spreadsheet opens. The UTF-8 BOM Excel needs is not here: it belongs to a
/// file, not to the text, and [`ExportFormat::file_byte_order_mark`] adds it on
/// the way to disk.
fn render_separated(grid: &ExportGrid, separator: char) -> String {
    let line_ending = csv_line_ending();
    let mut out = String::with_capacity(grid.rows.len() * 20 + grid.columns.len() * 16 + 4);
    for (index, column) in grid.columns.iter().enumerate() {
        if index > 0 {
            out.push(separator);
        }
        // The header is names, never values, so it carries no NULL signal.
        out.push_str(&escape_delimited_field(column, separator, false));
    }
    out.push_str(line_ending);

    for row in &grid.rows {
        for index in 0..grid.columns.len() {
            if index > 0 {
                out.push(separator);
            }
            let (text, force_quotes) = grid.display_cell(row, index);
            out.push_str(&escape_delimited_field(text, separator, force_quotes));
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

/// Whether `field` cannot be written bare in a delimited file.
///
/// The ONE statement of that rule. CSV and TSV differ only in their separator,
/// and the reader has to ask the same question the writer did — a NULL text
/// that must be quoted anyway carries no signal — so the two sides share this
/// rather than each spelling out a list of characters.
pub(crate) fn delimited_field_needs_quotes(field: &str, separator: char) -> bool {
    field.contains(separator) || field.contains('"') || field.contains('\n') || field.contains('\r')
}

/// Escape one field for a delimited file.
///
/// `force_quotes` writes quotes the grammar does not require, which is how a
/// value that spells the NULL text is kept apart from a NULL — see
/// [`ExportGrid::display_cell`]. Every reader treats `"x"` and `x` as the same
/// string, so nothing outside this app sees a difference.
pub(crate) fn escape_delimited_field(field: &str, separator: char, force_quotes: bool) -> String {
    if force_quotes || delimited_field_needs_quotes(field, separator) {
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
    escape_delimited_field(field, '\t', false)
}

fn render_json(grid: &ExportGrid) -> String {
    if grid.rows.is_empty() {
        return "[]\n".to_string();
    }
    // Two object keys with one name are one key to every reader, this app's own
    // importer included, so the writer makes them unique.
    let names = unique_field_names(grid.columns.clone());
    let mut out = String::from("[\n");
    for (row_index, row) in grid.rows.iter().enumerate() {
        out.push_str("  {\n");
        for (index, column) in names.iter().enumerate() {
            out.push_str("    ");
            out.push_str(&json_string(column));
            out.push_str(": ");
            out.push_str(&json_value(grid, row, index));
            if index + 1 < names.len() {
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
fn json_value(grid: &ExportGrid, row: &[ExportCell], index: usize) -> String {
    let Some(value) = grid.cell(row, index) else {
        return "null".to_string();
    };
    let value = value.as_str();
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
    // Sanitize first, then make unique: sanitizing CREATES collisions of its
    // own (`A(B` and `A)B` both become `A_B`), and an element name that repeats
    // inside one row is a column no reader can address.
    let names = unique_field_names(
        grid.columns
            .iter()
            .enumerate()
            .map(|(index, column)| xml_element_name(column, index))
            .collect(),
    );

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<results>\n");
    for row in &grid.rows {
        out.push_str("  <row>\n");
        for (index, name) in names.iter().enumerate() {
            match grid.cell(row, index) {
                None => out.push_str(&format!("    <{name}/>\n")),
                Some(value) => out.push_str(&format!(
                    "    <{name}>{}</{name}>\n",
                    escape_xml_text(value)
                )),
            }
        }
        out.push_str("  </row>\n");
    }
    out.push_str("</results>\n");
    out
}

/// Make emitted field names unique.
///
/// XML elements and JSON object keys are addressed BY NAME, so two columns that
/// end up sharing one are one column to every reader — this app's own importer
/// silently kept the first and dropped the rest. Duplicate result column names
/// are ordinary (`SELECT a.id, b.id FROM …`), so uniqueness is the writer's job
/// and belongs in one place both formats go through.
///
/// The first use of a name keeps it; every later one takes the lowest free
/// `_<n>` suffix, counting from 2.
fn unique_field_names(names: Vec<String>) -> Vec<String> {
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    names
        .into_iter()
        .map(|name| {
            if taken.insert(name.clone()) {
                return name;
            }
            let mut suffix = 2usize;
            loop {
                let candidate = format!("{name}_{suffix}");
                if taken.insert(candidate.clone()) {
                    return candidate;
                }
                suffix += 1;
            }
        })
        .collect()
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
            // An HTML table cell has no way to say "no value", so NULL and the
            // empty string are both an empty cell here — the one place this
            // format cannot keep them apart.
            let cell = match grid.cell(row, index) {
                None => String::new(),
                Some(value) => escape_html_text(value),
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
            .map(|index| match grid.cell(row, index) {
                // As in HTML, an empty cell is the only spelling Markdown has
                // for "no value", and the empty string shares it.
                None => String::new(),
                Some(value) => escape_markdown_cell(value),
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    out
}

/// A Markdown table cell cannot contain a raw `|` or a line break, so the pipe
/// is escaped and every line break becomes `<br>`.
///
/// `<` is escaped too, and that is what makes the reader an exact inverse: the
/// line break marker is written with a BARE `<`, so a `<br>` that was already in
/// the data (`\<br>`) can no longer be mistaken for one this function added.
/// Escaping runs first and the marker is introduced last, in that order, because
/// the marker must survive it. A `\<` is also how CommonMark spells a literal
/// `<`, so the rendered table shows the data rather than an HTML tag.
fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('<', "\\<")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose second row's NAME is SQL NULL.
    ///
    /// The fixture used to spell that as the TEXT `NULL` and let each
    /// serializer re-derive it. It is `None` now — the same fact, stated once
    /// where it is known — and CSV still writes the NULL display text for it,
    /// because CSV is a dump of what the grid shows.
    fn grid() -> ExportGrid {
        ExportGrid {
            columns: vec!["ID".to_string(), "NAME".to_string()],
            column_kinds: vec![SqlValueKind::Number, SqlValueKind::String],
            rows: vec![
                vec![Some("1".to_string()), Some("alpha".to_string())],
                vec![Some("2".to_string()), None],
            ],
            null_text: "NULL".to_string(),
        }
    }

    /// Rows of plain values, for the tests that never involve a NULL.
    fn cells(rows: &[&[&str]]) -> Vec<Vec<ExportCell>> {
        rows.iter()
            .map(|row| row.iter().map(|value| Some((*value).to_string())).collect())
            .collect()
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
        let csv = |field| escape_delimited_field(field, ',', false);
        assert_eq!(csv("a,b"), "\"a,b\"");
        assert_eq!(csv("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv("line1\rline2"), "\"line1\rline2\"");
        assert_eq!(csv("plain\ttext"), "plain\ttext");
    }

    #[test]
    fn tsv_quotes_tabs_but_not_commas() {
        assert_eq!(escape_tab_separated_field("a,b"), "a,b");
        assert_eq!(escape_tab_separated_field("a\tb"), "\"a\tb\"");
    }

    /// A VALUE that spells the NULL text is quoted; a NULL is not.
    ///
    /// The one signal a delimited file has for the difference. Without it this
    /// app's own export wrote the same bytes for both, and the importer — which
    /// can only read the bytes — turned the string into SQL NULL.
    #[test]
    fn a_value_that_spells_the_null_text_is_quoted_and_a_null_is_not() {
        let grid = ExportGrid {
            columns: vec!["V".to_string()],
            column_kinds: vec![SqlValueKind::String],
            rows: vec![
                vec![None],
                vec![Some("NULL".to_string())],
                vec![Some("plain".to_string())],
                vec![Some(String::new())],
            ],
            null_text: "NULL".to_string(),
        };
        // The empty value is quoted too: one column of it would otherwise be a
        // line with nothing on it, which no reader counts as a row.
        let line_ending = csv_line_ending();
        let expected = format!(
            "V{line_ending}NULL{line_ending}\"NULL\"{line_ending}plain{line_ending}\"\"{line_ending}"
        );
        assert_eq!(render(ExportFormat::Csv, &grid), expected);
        // TSV asks the same question with its own separator.
        assert_eq!(render(ExportFormat::Tsv, &grid), expected);
    }

    /// The signal costs nothing when the NULL text needs no quotes, and does
    /// not exist when it does — a NULL text holding the separator is quoted
    /// whichever it means, so neither spelling is free to mean the other.
    #[test]
    fn a_null_text_that_needs_quotes_carries_no_signal() {
        assert!(!delimited_field_needs_quotes("NULL", ','));
        assert!(!delimited_field_needs_quotes("", ','));
        assert!(delimited_field_needs_quotes("a,b", ','));
        assert!(!delimited_field_needs_quotes("a,b", '\t'));
        assert!(delimited_field_needs_quotes("a\tb", '\t'));

        let grid = ExportGrid {
            columns: vec!["V".to_string()],
            column_kinds: vec![SqlValueKind::String],
            rows: vec![vec![None], vec![Some("a,b".to_string())]],
            null_text: "a,b".to_string(),
        };
        let line_ending = csv_line_ending();
        assert_eq!(
            render(ExportFormat::Csv, &grid),
            format!("V{line_ending}\"a,b\"{line_ending}\"a,b\"{line_ending}"),
            "both are quoted, so the file cannot tell them apart — and says so \
             by reading both as NULL"
        );
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
            // A zero-padded code and a bare `.5`, which the JSON grammar does
            // not accept as a number. (It is not what either Oracle driver
            // writes for 0.5 — ODPI-C spells the leading zero itself
            // (`dpiDataBuffer__fromOracleNumberAsText`) and the thin decoder
            // inserts one, so both say `0.5`. It is here as text no reader
            // should silently turn into a number.) `1.2E+10` is deliberately
            // alongside them because exponent notation *is* a JSON number and
            // must stay unquoted.
            rows: cells(&[&["00123"], &[".5"], &["1.2E+10"]]),
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
            rows: cells(&[&["a\tb\u{1}"]]),
            ..ExportGrid::default()
        };
        assert!(render(ExportFormat::Json, &control).contains("\"a\\tb\\u0001\""));
    }

    #[test]
    fn xml_uses_an_empty_element_for_null_and_escapes_markup() {
        let markup = ExportGrid {
            columns: vec!["A".to_string(), "B".to_string()],
            // `B` is SQL NULL; `A` is a value that happens to hold markup.
            rows: vec![vec![Some("x & <y>".to_string()), None]],
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
            rows: vec![vec![Some("1 & 2".to_string())], vec![None]],
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

    /// The empty string is a value, and so is the text `NULL`.
    ///
    /// Every structured writer used to ask the grid's cell-EDITOR question —
    /// "would a user typing this mean NULL?" — which is generously true for an
    /// empty box and for every spelling of `null`. On the MySQL family both are
    /// real values, and folding them into SQL NULL rewrote the data on its way
    /// out. NULL is carried as an absent cell now, so no text is examined.
    #[test]
    fn text_that_reads_like_a_null_is_not_one() {
        let grid = ExportGrid {
            columns: vec!["V".to_string()],
            column_kinds: vec![SqlValueKind::String],
            rows: vec![
                vec![None],
                vec![Some(String::new())],
                vec![Some("NULL".to_string())],
                vec![Some("null".to_string())],
            ],
            null_text: "NULL".to_string(),
        };

        // JSON: only the absent cell is `null`.
        let json = render(ExportFormat::Json, &grid);
        assert_eq!(json.matches(": null").count(), 1, "{json}");
        assert!(json.contains("\"V\": \"\""), "{json}");
        assert!(json.contains("\"V\": \"NULL\""), "{json}");
        assert!(json.contains("\"V\": \"null\""), "{json}");

        // XML: `<V/>` is NULL, `<V></V>` is the empty string.
        let xml = render(ExportFormat::Xml, &grid);
        assert_eq!(xml.matches("<V/>").count(), 1, "{xml}");
        assert_eq!(xml.matches("<V></V>").count(), 1, "{xml}");
        assert!(xml.contains("<V>NULL</V>"), "{xml}");

        // CSV is a dump of what the grid SHOWS, so a NULL wears its display
        // text there. A VALUE that happens to spell that same text is QUOTED,
        // which is what keeps this format's promise too: it used to write the
        // identical bytes for both, and the reader could only guess — it chose
        // NULL, so the string was lost on the app's own round trip.
        let line_ending = csv_line_ending();
        assert_eq!(
            render(ExportFormat::Csv, &grid),
            format!(
                "V{line_ending}NULL{line_ending}\"\"{line_ending}\"NULL\"{line_ending}null{line_ending}"
            )
        );
    }

    /// An export reports the rows it WROTE, never the rows it was offered.
    ///
    /// `SQL Inserts` has three ways to write nothing, and the count used to
    /// come from the input — so an empty file was announced as a full export
    /// and an empty clipboard as "Copied N INSERT statements".
    #[test]
    fn an_export_reports_the_rows_it_wrote() {
        use crate::ui::grid_sql_export::{GridSqlSelection, SqlWriteDialect};

        let grid = ExportGrid {
            columns: vec!["A".to_string()],
            column_kinds: vec![SqlValueKind::Number],
            rows: vec![vec![Some("1".to_string())], vec![Some("2".to_string())]],
            null_text: "NULL".to_string(),
        };
        let selection = |selected_columns: Vec<usize>| GridSqlSelection {
            dialect: SqlWriteDialect::family_default(crate::db::DatabaseType::Oracle),
            table: Some("T".to_string()),
            all_columns: grid.columns.clone(),
            column_kinds: grid.column_kinds.clone(),
            selected_columns,
            rows: grid.rows.clone(),
        };

        let written =
            render_export_content(ExportFormat::SqlInserts, &grid, Some(&selection(vec![0])));
        assert_eq!(written.rows(), 2);
        assert_eq!(written.refusal(), None);

        // A table whose every column the server computes offers none, which is
        // a refusal — not a file of nothing reported as two rows.
        let refused = render_export_content(
            ExportFormat::SqlInserts,
            &grid,
            Some(&selection(Vec::new())),
        );
        assert_eq!(refused.rows(), 0);
        assert!(refused.text().is_empty());
        assert!(refused.refusal().is_some());

        // A data format writes every row it was given, as it always did.
        for format in [ExportFormat::Csv, ExportFormat::Json, ExportFormat::Xml] {
            assert_eq!(render_export_content(format, &grid, None).rows(), 2);
        }
    }

    /// A column name that repeats — or that only repeats once XML has
    /// sanitized it — must still address one column each.
    #[test]
    fn emitted_field_names_are_made_unique() {
        assert_eq!(
            unique_field_names(vec![
                "ID".to_string(),
                "ID".to_string(),
                "ID".to_string(),
                "ID_2".to_string(),
            ]),
            vec![
                "ID".to_string(),
                "ID_2".to_string(),
                "ID_3".to_string(),
                "ID_2_2".to_string(),
            ]
        );
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
