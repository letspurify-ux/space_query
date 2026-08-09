//! Parse an exported file back into rows a table can be loaded with.
//!
//! This is the inverse of [`crate::ui::result_export`], and it reads every
//! format that module writes: CSV, TSV, JSON, XML, HTML, Markdown, and a file
//! of `INSERT` statements. Pure functions over text — no FLTK, no database —
//! so an export/import round trip is unit-testable byte for byte.
//!
//! ## How each format says "NULL"
//!
//! The exporter uses each format's own vocabulary, so the importer has to read
//! each one back the same way. These rules are the exact inverse of what
//! [`crate::ui::result_export::render`] writes:
//!
//! | Format | NULL is |
//! | --- | --- |
//! | CSV, TSV | a cell whose text equals the configured NULL text |
//! | JSON | the `null` literal |
//! | XML | an empty element written `<C/>` (`<C></C>` is the empty string) |
//! | HTML, Markdown | an empty cell |
//! | SQL Inserts | the `NULL` keyword |
//!
//! Nothing here guesses a type: every cell arrives as text (or NULL), and the
//! target column's own type decides how it becomes a literal. That work belongs
//! to [`crate::ui::table_import`].

use std::path::Path;

use crate::ui::result_export::ExportFormat;

/// What one cell of an imported file holds: text, or SQL NULL.
pub type ImportCell = Option<String>;

/// A parsed file: column names and rows of cells, both in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportedTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<ImportCell>>,
}

/// How to read the file. Only the fields the chosen format actually uses are
/// consulted — see [`ExportFormat`] helpers below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    pub format: ExportFormat,
    /// Whether the first row names the columns. Ignored by the formats that
    /// carry their column names inside the data.
    pub has_header: bool,
    /// The text that means SQL NULL in CSV and TSV. Empty means "an empty cell
    /// is NULL".
    pub null_text: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Csv,
            has_header: true,
            // What `SessionState` shows for NULL out of the box, so a file this
            // app exported reads back with its NULLs intact.
            null_text: "NULL".to_string(),
        }
    }
}

/// Whether a "first row is the header" choice means anything for this format.
///
/// JSON, XML and SQL name their columns inside the data. A Markdown table's
/// grammar puts the header above the mandatory `---` separator, so it is always
/// present.
pub fn header_choice_applies(format: ExportFormat) -> bool {
    matches!(
        format,
        ExportFormat::Csv | ExportFormat::Tsv | ExportFormat::Html
    )
}

/// Whether a NULL-text choice means anything for this format.
pub fn null_text_choice_applies(format: ExportFormat) -> bool {
    matches!(format, ExportFormat::Csv | ExportFormat::Tsv)
}

/// The format a file's extension implies, if any.
pub fn detect_format(path: &Path) -> Option<ExportFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    ExportFormat::ALL
        .into_iter()
        .find(|format| format.extension() == extension)
        .or(match extension.as_str() {
            "txt" => Some(ExportFormat::Csv),
            "text" | "markdown" => Some(ExportFormat::Markdown),
            "htm" => Some(ExportFormat::Html),
            _ => None,
        })
}

/// Parse `text` into columns and rows.
pub fn parse(text: &str, options: &ImportOptions) -> Result<ImportedTable, String> {
    // A UTF-8 BOM is what `ExportFormat::file_byte_order_mark` puts in front of
    // a spreadsheet file; it is data to nobody.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let table = match options.format {
        ExportFormat::Csv => parse_separated(text, ',', options)?,
        ExportFormat::Tsv => parse_separated(text, '\t', options)?,
        ExportFormat::Json => parse_json(text)?,
        ExportFormat::Xml => parse_xml(text)?,
        ExportFormat::Html => parse_html(text, options.has_header)?,
        ExportFormat::Markdown => parse_markdown(text)?,
        ExportFormat::SqlInserts => parse_sql_inserts(text)?,
    };
    validate(table)
}

/// Reject a parse that produced nothing usable, and pad short rows so every row
/// is as wide as the header.
fn validate(mut table: ImportedTable) -> Result<ImportedTable, String> {
    if table.columns.is_empty() {
        return Err("The file has no columns to import.".to_string());
    }
    if table.columns.iter().any(|column| column.trim().is_empty()) {
        return Err("The file has a column with no name.".to_string());
    }
    let width = table.columns.len();
    for row in &mut table.rows {
        if row.len() > width {
            return Err(format!(
                "A row has {} values but the file declares {width} columns.",
                row.len()
            ));
        }
        row.resize(width, None);
    }
    Ok(table)
}

fn header_or_generated(
    first: Vec<ImportCell>,
    has_header: bool,
) -> (Vec<String>, Option<Vec<ImportCell>>) {
    if has_header {
        let columns = first
            .into_iter()
            .map(|cell| cell.unwrap_or_default())
            .collect();
        (columns, None)
    } else {
        let columns = (1..=first.len())
            .map(|index| format!("COLUMN_{index}"))
            .collect();
        (columns, Some(first))
    }
}

// ---------------------------------------------------------------------------
// CSV / TSV
// ---------------------------------------------------------------------------

/// Read RFC 4180 records: quoted fields may hold the separator, a line break,
/// or a doubled quote. Records end at LF, CRLF, or a lone CR.
fn split_delimited_records(text: &str, separator: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    // Whether anything of the current field has been read yet. A quote only
    // opens a quoted field at the very start of one: `ab"cd` is the literal
    // text `ab"cd`, which is how a spreadsheet reads it too.
    let mut field_started = false;
    let mut record_started = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' if !field_started => {
                field_started = true;
                record_started = true;
                in_quotes = true;
            }
            _ if ch == separator => {
                record.push(std::mem::take(&mut field));
                field_started = false;
                record_started = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                field_started = false;
                record_started = false;
            }
            _ => {
                field_started = true;
                record_started = true;
                field.push(ch);
            }
        }
    }
    if record_started || !field.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

fn parse_separated(
    text: &str,
    separator: char,
    options: &ImportOptions,
) -> Result<ImportedTable, String> {
    let records = split_delimited_records(text, separator);
    let mut records = records.into_iter();
    let Some(first) = records.next() else {
        return Err("The file is empty.".to_string());
    };
    let to_cells = |record: Vec<String>| -> Vec<ImportCell> {
        record
            .into_iter()
            .map(|value| {
                if value == options.null_text {
                    None
                } else {
                    Some(value)
                }
            })
            .collect()
    };

    // The header line is text even when it happens to equal the NULL text.
    let first_cells: Vec<ImportCell> = if options.has_header {
        first.into_iter().map(Some).collect()
    } else {
        to_cells(first)
    };
    let (columns, leading_row) = header_or_generated(first_cells, options.has_header);
    let mut rows: Vec<Vec<ImportCell>> = leading_row.into_iter().collect();
    rows.extend(records.map(to_cells));
    Ok(ImportedTable { columns, rows })
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn parse_json(text: &str) -> Result<ImportedTable, String> {
    let records: Vec<OrderedObject> =
        serde_json::from_str(text).map_err(|error| format!("The JSON is not valid: {error}"))?;

    let mut columns: Vec<String> = Vec::new();
    for record in &records {
        for (key, _) in &record.0 {
            if !columns.iter().any(|column| column == key) {
                columns.push(key.clone());
            }
        }
    }

    let rows = records
        .iter()
        .map(|record| {
            columns
                .iter()
                .map(|column| {
                    record
                        .0
                        .iter()
                        .find(|(key, _)| key == column)
                        .and_then(|(_, value)| json_cell(value))
                })
                .collect()
        })
        .collect();
    Ok(ImportedTable { columns, rows })
}

/// One JSON object with its keys still in document order and its values still
/// in their original spelling.
///
/// `serde_json::Value` stores an object in a `BTreeMap`, which would alphabetize
/// the columns, and it stores a number as an `f64`, which would rewrite
/// `1.2E+10` as `12000000000.0`. Reading entries off the map visitor as
/// `RawValue` keeps both.
struct OrderedObject(Vec<(String, Box<serde_json::value::RawValue>)>);

impl<'de> serde::Deserialize<'de> for OrderedObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ObjectVisitor;

        impl<'de> serde::de::Visitor<'de> for ObjectVisitor {
            type Value = OrderedObject;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some(entry) =
                    map.next_entry::<String, Box<serde_json::value::RawValue>>()?
                {
                    entries.push(entry);
                }
                Ok(OrderedObject(entries))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

/// A JSON value as cell text.
///
/// A string is decoded to its characters. Everything else — number, boolean,
/// nested object or array — keeps the exact text the file carried, so a value
/// is never silently reformatted on its way into a column.
fn json_cell(value: &serde_json::value::RawValue) -> ImportCell {
    let raw = value.get().trim();
    if raw == "null" {
        return None;
    }
    if raw.starts_with('"') {
        return serde_json::from_str::<String>(raw).ok();
    }
    Some(raw.to_string())
}

// ---------------------------------------------------------------------------
// Markup: a tolerant tree used by both the XML and the HTML reader
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum MarkupContent {
    Text(String),
    Element(MarkupNode),
}

#[derive(Clone, Debug, Default)]
struct MarkupNode {
    name: String,
    /// Written `<name/>`, which is how the XML export spells NULL.
    self_closing: bool,
    /// Text and child elements in document order, so a value split around a
    /// nested element reads back the way it was written.
    content: Vec<MarkupContent>,
}

impl MarkupNode {
    fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    fn elements(&self) -> impl Iterator<Item = &MarkupNode> {
        self.content.iter().filter_map(|item| match item {
            MarkupContent::Element(node) => Some(node),
            MarkupContent::Text(_) => None,
        })
    }

    /// Text of this element and everything under it, in document order.
    fn all_text(&self) -> String {
        let mut out = String::new();
        for item in &self.content {
            match item {
                MarkupContent::Text(text) => out.push_str(text),
                MarkupContent::Element(child) => {
                    // A line break element is the only markup the HTML export
                    // could have put inside a cell; it stands for a newline.
                    if child.is("br") {
                        out.push('\n');
                    }
                    out.push_str(&child.all_text());
                }
            }
        }
        out
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.content.last_mut() {
            Some(MarkupContent::Text(existing)) => existing.push_str(text),
            _ => self.content.push(MarkupContent::Text(text.to_string())),
        }
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a MarkupNode> {
        self.elements().filter(move |child| child.is(name))
    }
}

/// HTML elements that never have a closing tag. Only the ones that can appear
/// in or around a table matter.
const VOID_ELEMENTS: [&str; 8] = ["br", "hr", "img", "input", "meta", "link", "col", "source"];

/// HTML table elements a browser closes implicitly when a sibling opens.
/// `<tr><td>a<td>b</tr>` is two cells, not one nested inside the other.
const IMPLICITLY_CLOSED: [(&str, &[&str]); 3] = [
    ("td", &["td", "th"]),
    ("th", &["td", "th"]),
    ("tr", &["tr"]),
];

/// Build a forest from markup text.
///
/// Deliberately forgiving: a close tag that does not match the open element
/// closes everything up to the nearest matching ancestor and is otherwise
/// ignored, and unclosed elements are closed at end of input. That is what lets
/// the same reader handle strict XML and the looser HTML a browser would save.
fn parse_markup(text: &str) -> Vec<MarkupNode> {
    let mut roots: Vec<MarkupNode> = Vec::new();
    let mut stack: Vec<MarkupNode> = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    fn finish(stack: &mut [MarkupNode], roots: &mut Vec<MarkupNode>, node: MarkupNode) {
        match stack.last_mut() {
            Some(parent) => parent.content.push(MarkupContent::Element(node)),
            None => roots.push(node),
        }
    }
    fn close_to(stack: &mut Vec<MarkupNode>, roots: &mut Vec<MarkupNode>, position: usize) {
        while stack.len() > position {
            let Some(node) = stack.pop() else { break };
            finish(stack, roots, node);
        }
    }

    while index < bytes.len() {
        if bytes[index] != '<' {
            let start = index;
            while index < bytes.len() && bytes[index] != '<' {
                index += 1;
            }
            let raw: String = bytes[start..index].iter().collect();
            if let Some(top) = stack.last_mut() {
                top.push_text(&decode_entities(&raw));
            }
            continue;
        }

        // `<!--`, `<![CDATA[`, `<!DOCTYPE`, `<?xml`: markup that is not an element.
        if let Some(after) = skip_non_element(&bytes, index, &mut stack) {
            index = after;
            continue;
        }

        let Some(end) = find_tag_end(&bytes, index) else {
            // An unterminated `<` is text.
            let raw: String = bytes[index..].iter().collect();
            if let Some(top) = stack.last_mut() {
                top.push_text(&decode_entities(&raw));
            }
            break;
        };
        let inner: String = bytes[index + 1..end].iter().collect();
        index = end + 1;

        if let Some(name) = inner.strip_prefix('/') {
            let name = tag_name(name);
            if let Some(position) = stack.iter().rposition(|node| node.is(&name)) {
                close_to(&mut stack, &mut roots, position);
            }
            continue;
        }

        let self_closing = inner.trim_end().ends_with('/');
        let name = tag_name(&inner);
        if name.is_empty() {
            continue;
        }
        if let Some((_, closes)) = IMPLICITLY_CLOSED
            .iter()
            .find(|(tag, _)| name.eq_ignore_ascii_case(tag))
        {
            if let Some(position) = stack
                .iter()
                .rposition(|node| closes.iter().any(|tag| node.is(tag)))
            {
                close_to(&mut stack, &mut roots, position);
            }
        }
        let node = MarkupNode {
            self_closing,
            name,
            ..MarkupNode::default()
        };
        if self_closing || VOID_ELEMENTS.iter().any(|void| node.is(void)) {
            finish(&mut stack, &mut roots, node);
        } else {
            stack.push(node);
        }
    }

    close_to(&mut stack, &mut roots, 0);
    roots
}

/// Consume a comment, CDATA section, doctype, or processing instruction that
/// starts at `index`. CDATA contributes its contents as literal text.
fn skip_non_element(bytes: &[char], index: usize, stack: &mut [MarkupNode]) -> Option<usize> {
    let starts_with = |prefix: &str| {
        let prefix: Vec<char> = prefix.chars().collect();
        bytes.len() >= index + prefix.len() && bytes[index..index + prefix.len()] == prefix[..]
    };
    let find = |needle: &str, from: usize| {
        let needle: Vec<char> = needle.chars().collect();
        (from..bytes.len().saturating_sub(needle.len().saturating_sub(1)))
            .find(|start| bytes[*start..start + needle.len()] == needle[..])
    };

    if starts_with("<!--") {
        return Some(find("-->", index + 4).map_or(bytes.len(), |at| at + 3));
    }
    if starts_with("<![CDATA[") {
        let start = index + 9;
        let end = find("]]>", start).unwrap_or(bytes.len());
        let raw: String = bytes[start..end.min(bytes.len())].iter().collect();
        if let Some(top) = stack.last_mut() {
            top.push_text(&raw);
        }
        return Some((end + 3).min(bytes.len()));
    }
    if starts_with("<!") || starts_with("<?") {
        return Some(find(">", index).map_or(bytes.len(), |at| at + 1));
    }
    None
}

/// The index of the `>` that closes the tag opened at `index`, skipping any
/// `>` inside a quoted attribute value.
fn find_tag_end(bytes: &[char], index: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, ch) in bytes.iter().enumerate().skip(index + 1) {
        match (quote, ch) {
            (Some(open), ch) if *ch == open => quote = None,
            (None, '"') | (None, '\'') => quote = Some(*ch),
            (None, '>') => return Some(offset),
            _ => {}
        }
    }
    None
}

fn tag_name(inner: &str) -> String {
    inner
        .trim_start()
        .trim_start_matches('/')
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '/' && *ch != '>')
        .collect()
}

/// Resolve the entities the exporter writes, plus the numeric forms and the
/// handful of named ones a hand-written file is likely to carry.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let resolved = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{A0}'),
            _ => entity
                .strip_prefix('#')
                .and_then(|number| match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match resolved {
            Some(ch) => {
                out.push(ch);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// Read `<results><row><COL>value</COL>…</row>…</results>`.
///
/// The element names are not fixed: the first element is the document, its
/// children are rows, and their children are columns. That reads this app's own
/// export and any XML shaped the same way.
fn parse_xml(text: &str) -> Result<ImportedTable, String> {
    let roots = parse_markup(text);
    let Some(document) = roots.into_iter().find(|node| !node.name.is_empty()) else {
        return Err("The XML has no elements.".to_string());
    };
    let row_nodes: Vec<&MarkupNode> = document
        .elements()
        .filter(|node| !node.name.is_empty())
        .collect();
    if row_nodes.is_empty() {
        return Err("The XML has no row elements.".to_string());
    }

    let mut columns: Vec<String> = Vec::new();
    for row in &row_nodes {
        for cell in row.elements() {
            if !columns.iter().any(|column| column == &cell.name) {
                columns.push(cell.name.clone());
            }
        }
    }
    if columns.is_empty() {
        return Err("The XML rows have no column elements.".to_string());
    }

    let rows = row_nodes
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|column| {
                    row.children_named(column).next().and_then(|cell| {
                        // `<C/>` is NULL; `<C></C>` is the empty string.
                        (!cell.self_closing).then(|| cell.all_text())
                    })
                })
                .collect()
        })
        .collect();
    Ok(ImportedTable { columns, rows })
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

fn parse_html(text: &str, has_header: bool) -> Result<ImportedTable, String> {
    let roots = parse_markup(text);
    let table =
        find_html_table(roots.iter()).ok_or_else(|| "The HTML has no <table>.".to_string())?;

    let mut rows: Vec<(bool, Vec<ImportCell>)> = Vec::new();
    collect_html_rows(table, &mut rows);
    if rows.is_empty() {
        return Err("The HTML table has no rows.".to_string());
    }

    // A row of `<th>` is the header whatever the checkbox says; a table of only
    // `<td>` leaves the decision to the caller.
    let header_first = rows[0].0 || has_header;
    let mut rows = rows.into_iter().map(|(_, cells)| cells);
    let Some(first) = rows.next() else {
        return Err("The HTML table has no rows.".to_string());
    };
    let (columns, leading_row) = header_or_generated(first, header_first);
    let mut data: Vec<Vec<ImportCell>> = leading_row.into_iter().collect();
    data.extend(rows);
    Ok(ImportedTable {
        columns,
        rows: data,
    })
}

fn find_html_table<'a>(nodes: impl Iterator<Item = &'a MarkupNode>) -> Option<&'a MarkupNode> {
    for node in nodes {
        if node.is("table") {
            return Some(node);
        }
        if let Some(found) = find_html_table(node.elements()) {
            return Some(found);
        }
    }
    None
}

/// Gather `<tr>` rows from a table, descending through `<thead>`/`<tbody>`.
/// The flag says whether the row was written with `<th>` cells.
fn collect_html_rows(node: &MarkupNode, rows: &mut Vec<(bool, Vec<ImportCell>)>) {
    for child in node.elements() {
        if child.is("tr") {
            let mut header = false;
            let mut cells: Vec<ImportCell> = Vec::new();
            for cell in child.elements() {
                if !cell.is("td") && !cell.is("th") {
                    continue;
                }
                header |= cell.is("th");
                let text = cell.all_text();
                // The exporter writes NULL as an empty cell.
                cells.push((!text.is_empty()).then_some(text));
            }
            if !cells.is_empty() {
                rows.push((header, cells));
            }
        } else if !child.is("table") {
            collect_html_rows(child, rows);
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

fn parse_markdown(text: &str) -> Result<ImportedTable, String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'));
    let Some(header) = lines.next() else {
        return Err("The file has no Markdown table.".to_string());
    };
    let columns: Vec<String> = split_markdown_row(header)
        .into_iter()
        .map(|cell| cell.unwrap_or_default())
        .collect();

    // The `| --- |` line is only legal directly under the header, so only that
    // one line is skipped. A later row of dashes is data.
    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        if index == 0 && is_markdown_separator(line) {
            continue;
        }
        rows.push(split_markdown_row(line));
    }
    Ok(ImportedTable { columns, rows })
}

/// The `| --- | --- |` line that separates a Markdown header from its body.
fn is_markdown_separator(line: &str) -> bool {
    let cells = split_markdown_row(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.as_deref().unwrap_or("").trim();
            !cell.is_empty()
                && cell
                    .trim_start_matches(':')
                    .trim_end_matches(':')
                    .chars()
                    .all(|ch| ch == '-')
        })
}

/// Split one `| a | b |` line, undoing `escape_markdown_cell`.
fn split_markdown_row(line: &str) -> Vec<ImportCell> {
    let line = line.trim();
    let inner = line
        .strip_prefix('|')
        .unwrap_or(line)
        .strip_suffix('|')
        .unwrap_or_else(|| line.strip_prefix('|').unwrap_or(line));

    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            // `\\` and `\|` are what the exporter writes for a literal
            // backslash and pipe; any other backslash is itself.
            '\\' => match chars.next() {
                Some(next @ ('\\' | '|')) => cell.push(next),
                Some(next) => {
                    cell.push('\\');
                    cell.push(next);
                }
                None => cell.push('\\'),
            },
            '|' => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(ch),
        }
    }
    cells.push(cell);

    cells
        .into_iter()
        .map(|cell| {
            let cell = cell.trim().replace("<br>", "\n");
            // The exporter writes NULL as an empty cell.
            (!cell.is_empty()).then_some(cell)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SQL Inserts
// ---------------------------------------------------------------------------

/// Read a file of `INSERT INTO t (a, b) VALUES (…);` statements.
///
/// Only the literal forms this app's own `SQL Inserts` export writes are
/// accepted, plus the conversion wrappers Oracle needs — `TO_DATE`,
/// `TO_TIMESTAMP`, `TO_TIMESTAMP_TZ`, `HEXTORAW` — which are unwrapped back to
/// the text the grid showed. Anything else is refused by name instead of being
/// quoted into a string that would mean something different.
///
/// The two dialects disagree about the backslash: MySQL and MariaDB write `\\`
/// for a literal one, Oracle writes it as itself. Each statement says which it
/// is by how it quotes its column names — backticks are the MySQL family's and
/// nobody else's — so a file exported from either backend reads back exactly,
/// whichever backend it is being imported into.
fn parse_sql_inserts(text: &str) -> Result<ImportedTable, String> {
    let statements = split_sql_statements(text);
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<ImportCell>> = Vec::new();

    for statement in statements {
        let Some((statement_columns, values, backslash_escapes)) =
            split_insert_statement(&statement)?
        else {
            continue;
        };
        if columns.is_empty() {
            columns = statement_columns;
        } else if columns != statement_columns {
            return Err("The INSERT statements do not all use the same column list.".to_string());
        }
        if values.len() != columns.len() {
            return Err(format!(
                "An INSERT lists {} columns but {} values.",
                columns.len(),
                values.len()
            ));
        }
        rows.push(
            values
                .iter()
                .map(|value| sql_value_text(value, backslash_escapes))
                .collect::<Result<Vec<_>, String>>()?,
        );
    }

    if columns.is_empty() {
        return Err("The file has no INSERT statement with a column list.".to_string());
    }
    Ok(ImportedTable { columns, rows })
}

/// Split on `;` outside string literals, comments, and parentheses.
fn split_sql_statements(text: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut depth = 0i32;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                current.push(ch);
                while let Some(inner) = chars.next() {
                    current.push(inner);
                    if inner == '\\' {
                        if let Some(escaped) = chars.next() {
                            current.push(escaped);
                        }
                    } else if inner == '\'' {
                        if chars.peek() == Some(&'\'') {
                            if let Some(doubled) = chars.next() {
                                current.push(doubled);
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
            '"' | '`' => {
                // A quoted identifier can hold a `;` or a paren just as a
                // string can.
                current.push(ch);
                for inner in chars.by_ref() {
                    current.push(inner);
                    if inner == ch {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        break;
                    }
                }
                current.push('\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for inner in chars.by_ref() {
                    if previous == '*' && inner == '/' {
                        break;
                    }
                    previous = inner;
                }
                current.push(' ');
            }
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ';' if depth <= 0 => {
                statements.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        statements.push(current);
    }
    statements
        .into_iter()
        .filter(|statement| !statement.trim().is_empty())
        .collect()
}

/// Pull the column list, the value list, and whether the statement is written
/// in the MySQL family's dialect out of one INSERT. `Ok(None)` means the
/// statement is not an INSERT and should be ignored.
type InsertParts = Option<(Vec<String>, Vec<String>, bool)>;

fn split_insert_statement(statement: &str) -> Result<InsertParts, String> {
    let trimmed = statement.trim();
    let mut words = trimmed.split_whitespace();
    if !words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("insert"))
    {
        return Ok(None);
    }
    let Some(columns_open) = find_open_paren(trimmed) else {
        return Err("An INSERT statement has no column list.".to_string());
    };
    // `INSERT INTO t VALUES (…)` has no column list, so there is nothing to map
    // the values onto. Say that instead of reading the values as column names.
    if trimmed[..columns_open]
        .split_whitespace()
        .next_back()
        .is_some_and(|word| word.eq_ignore_ascii_case("values"))
    {
        return Err(
            "An INSERT statement has no column list, so its values cannot be mapped to columns."
                .to_string(),
        );
    }
    let columns_close = matching_paren(trimmed, columns_open)
        .ok_or_else(|| "An INSERT column list is not closed.".to_string())?;
    let raw_columns = split_top_level(&trimmed[columns_open + 1..columns_close]);
    let backslash_escapes = raw_columns
        .iter()
        .any(|column| column.trim().starts_with('`'));
    let columns = raw_columns
        .into_iter()
        .map(|column| unquote_identifier(column.trim()))
        .collect::<Vec<_>>();

    let tail = &trimmed[columns_close + 1..];
    let values_at = find_keyword(tail, "values")
        .ok_or_else(|| "An INSERT statement has no VALUES list.".to_string())?;
    let values_open = tail[values_at..]
        .find('(')
        .map(|offset| values_at + offset)
        .ok_or_else(|| "An INSERT VALUES list is missing.".to_string())?;
    let values_close = matching_paren(tail, values_open)
        .ok_or_else(|| "An INSERT VALUES list is not closed.".to_string())?;
    let values = split_top_level(&tail[values_open + 1..values_close])
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect();
    Ok(Some((columns, values, backslash_escapes)))
}

/// The first `(` that is not inside a string literal or a quoted identifier —
/// a schema like `"SC(H)EMA"` must not be mistaken for the column list.
fn find_open_paren(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in text.char_indices() {
        match (quote, ch) {
            (Some(open), ch) if ch == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(ch),
            (None, '(') => return Some(index),
            (None, _) => {}
        }
    }
    None
}

fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    for (index, ch) in text.char_indices().skip(open) {
        if in_string {
            if ch == '\'' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a comma-separated list, ignoring commas inside strings or nested
/// parentheses.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        if in_string {
            if ch == '\'' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' => in_string = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Find `keyword` as a whole word outside string literals.
fn find_keyword(text: &str, keyword: &str) -> Option<usize> {
    let lowered = text.to_ascii_lowercase();
    let bytes = lowered.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = lowered[from..].find(keyword) {
        let at = from + offset;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric() && bytes[at - 1] != b'_';
        let after = at + keyword.len();
        let after_ok =
            after >= bytes.len() || !bytes[after].is_ascii_alphanumeric() && bytes[after] != b'_';
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + keyword.len();
    }
    None
}

fn unquote_identifier(name: &str) -> String {
    let name = name.trim();
    for quote in ['"', '`'] {
        if name.len() >= 2 && name.starts_with(quote) && name.ends_with(quote) {
            let inner = &name[1..name.len() - 1];
            return inner.replace(&format!("{quote}{quote}"), &quote.to_string());
        }
    }
    name.to_string()
}

/// Turn one SQL value expression back into cell text.
fn sql_value_text(value: &str, backslash_escapes: bool) -> Result<ImportCell, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    if let Some(text) = sql_string_literal_text(value, backslash_escapes) {
        return Ok(Some(text));
    }
    if is_sql_numeric_literal(value) {
        return Ok(Some(value.to_string()));
    }
    // A `SqlValueKind::Boolean` column exports its text unquoted, so `TRUE` and
    // `FALSE` come back the way they were written.
    if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
        return Ok(Some(value.to_string()));
    }
    if let Some(text) = unwrap_conversion_call(value, backslash_escapes)? {
        return Ok(Some(text));
    }
    Err(format!(
        "The SQL file has a value this import cannot read: {value}"
    ))
}

/// Decode `'…'`, undoing the doubled quote every dialect uses and, when the
/// statement is MySQL-flavoured, the backslash escapes too.
fn sql_string_literal_text(value: &str, backslash_escapes: bool) -> Option<String> {
    let inner = value.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                // A lone quote inside the literal would have ended it, so this
                // is the first half of a doubled pair.
                chars.next()?;
                out.push('\'');
            }
            '\\' if backslash_escapes => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('0') => out.push('\0'),
                Some(next) => out.push(next),
                None => return None,
            },
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn is_sql_numeric_literal(value: &str) -> bool {
    let value = value.strip_prefix(['+', '-']).unwrap_or(value);
    if value.is_empty() {
        return false;
    }
    let mut mantissa = value;
    if let Some((head, exponent)) = value.split_once(['e', 'E']) {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !exponent.chars().all(|ch| ch.is_ascii_digit()) {
            return false;
        }
        mantissa = head;
    }
    let digits: Vec<&str> = mantissa.split('.').collect();
    digits.len() <= 2
        && digits.iter().any(|part| !part.is_empty())
        && digits
            .iter()
            .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
}

/// Unwrap the conversion calls the SQL export wraps Oracle values in, giving
/// back the text the grid displayed. `Ok(None)` means it is not one of them.
fn unwrap_conversion_call(value: &str, backslash_escapes: bool) -> Result<Option<String>, String> {
    let Some(open) = value.find('(') else {
        return Ok(None);
    };
    let name = value[..open].trim();
    let known = ["to_date", "to_timestamp", "to_timestamp_tz", "hextoraw"]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate));
    if !known {
        return Ok(None);
    }
    let close = matching_paren(value, open)
        .ok_or_else(|| format!("A conversion call is not closed: {value}"))?;
    if value[close + 1..].trim().is_empty() {
        // The first argument is the value; a format model, if present, is the
        // one the exporter chose and carries no data.
        if let Some(first) = split_top_level(&value[open + 1..close]).first() {
            if let Some(text) = sql_string_literal_text(first.trim(), backslash_escapes) {
                return Ok(Some(text));
            }
        }
    }
    Err(format!(
        "The SQL file has a value this import cannot read: {value}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DatabaseType, SqlValueKind};
    use crate::ui::grid_sql_export::{build_sql_inserts, GridSqlSelection};
    use crate::ui::result_export::{render, ExportGrid};

    const NULL_TEXT: &str = "NULL";

    /// A grid holding every shape that has ever broken a serializer: a
    /// separator, a quote, a newline, a lone CR, a pipe, a backslash, an
    /// entity, a zero-padded number, Korean text, and a NULL.
    fn hostile_grid() -> ExportGrid {
        ExportGrid {
            columns: vec!["ID".to_string(), "NAME".to_string(), "CODE".to_string()],
            column_kinds: vec![
                SqlValueKind::Number,
                SqlValueKind::String,
                SqlValueKind::String,
            ],
            rows: vec![
                vec![
                    "1".to_string(),
                    "a,b\t\"c\"\nd\re|f\\g<h>&i".to_string(),
                    "00123".to_string(),
                ],
                vec!["2".to_string(), "한글".to_string(), NULL_TEXT.to_string()],
            ],
            null_text: NULL_TEXT.to_string(),
        }
    }

    fn options(format: ExportFormat) -> ImportOptions {
        ImportOptions {
            format,
            ..ImportOptions::default()
        }
    }

    /// What `hostile_grid` means, as the importer should report it. Markdown
    /// and HTML cannot carry a lone CR, so the caller says what to expect
    /// there.
    fn expected(name: &str) -> ImportedTable {
        ImportedTable {
            columns: vec!["ID".to_string(), "NAME".to_string(), "CODE".to_string()],
            rows: vec![
                vec![
                    Some("1".to_string()),
                    Some(name.to_string()),
                    Some("00123".to_string()),
                ],
                vec![Some("2".to_string()), Some("한글".to_string()), None],
            ],
        }
    }

    fn round_trip(format: ExportFormat) -> ImportedTable {
        parse(&render(format, &hostile_grid()), &options(format))
            .unwrap_or_else(|error| panic!("{} failed to parse: {error}", format.label()))
    }

    #[test]
    fn csv_round_trips_every_hostile_value() {
        assert_eq!(
            round_trip(ExportFormat::Csv),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn tsv_round_trips_every_hostile_value() {
        assert_eq!(
            round_trip(ExportFormat::Tsv),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn json_round_trips_every_hostile_value() {
        assert_eq!(
            round_trip(ExportFormat::Json),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn xml_round_trips_every_hostile_value() {
        assert_eq!(
            round_trip(ExportFormat::Xml),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn html_round_trips_every_hostile_value() {
        assert_eq!(
            round_trip(ExportFormat::Html),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn markdown_round_trips_line_breaks_as_newlines() {
        // `escape_markdown_cell` writes every line break as `<br>`, so a lone
        // CR comes back as a newline. Everything else survives.
        assert_eq!(
            round_trip(ExportFormat::Markdown),
            expected("a,b\t\"c\"\nd\ne|f\\g<h>&i")
        );
    }

    #[test]
    fn a_utf8_byte_order_mark_is_not_data() {
        let text = format!(
            "{}{}",
            ExportFormat::Csv.file_byte_order_mark(),
            render(ExportFormat::Csv, &hostile_grid())
        );
        let table = parse(&text, &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.columns[0], "ID");
    }

    fn selection(db_type: DatabaseType) -> GridSqlSelection {
        let grid = hostile_grid();
        GridSqlSelection {
            db_type,
            table: Some("T".to_string()),
            all_columns: grid.columns.clone(),
            column_kinds: grid.column_kinds.clone(),
            selected_columns: (0..grid.columns.len()).collect(),
            rows: grid.rows.clone(),
            null_text: grid.null_text.clone(),
        }
    }

    #[test]
    fn sql_inserts_round_trip_on_oracle() {
        let sql = build_sql_inserts(&selection(DatabaseType::Oracle));
        assert_eq!(
            parse(&sql, &options(ExportFormat::SqlInserts)).expect("parses"),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn sql_inserts_round_trip_on_mysql() {
        // MySQL literals carry backslash escapes and backtick-quoted columns.
        let sql = build_sql_inserts(&selection(DatabaseType::MySQL));
        assert_eq!(
            parse(&sql, &options(ExportFormat::SqlInserts)).expect("parses"),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn sql_inserts_unwrap_the_oracle_conversion_calls() {
        let sql = "INSERT INTO T (HIRED, TS, TSZ, RAWC) VALUES (\
                   TO_DATE('1980-12-17 09:30:00','YYYY-MM-DD HH24:MI:SS'), \
                   TO_TIMESTAMP('1980-12-17 09:30:00.123456','YYYY-MM-DD HH24:MI:SS.FF'), \
                   TO_TIMESTAMP_TZ('1980-12-17 09:30:00 +09:00','YYYY-MM-DD HH24:MI:SS TZH:TZM'), \
                   HEXTORAW('DEADBEEF'));";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("1980-12-17 09:30:00".to_string()),
                Some("1980-12-17 09:30:00.123456".to_string()),
                Some("1980-12-17 09:30:00 +09:00".to_string()),
                Some("DEADBEEF".to_string()),
            ]]
        );
    }

    #[test]
    fn an_unreadable_sql_value_is_refused_by_name() {
        let sql = "INSERT INTO T (A) VALUES (SYSDATE);";
        let error = parse(sql, &options(ExportFormat::SqlInserts)).expect_err("refused");
        assert!(error.contains("SYSDATE"), "{error}");
    }

    #[test]
    fn sql_inserts_reject_a_changing_column_list() {
        let sql = "INSERT INTO T (A) VALUES (1);\nINSERT INTO T (B) VALUES (2);";
        assert!(parse(sql, &options(ExportFormat::SqlInserts)).is_err());
    }

    #[test]
    fn sql_inserts_ignore_other_statements_and_comments() {
        let sql = "-- a comment with a ; inside\nCOMMIT;\nINSERT INTO T (A) VALUES ('x;y');";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("x;y".to_string())]]);
    }

    #[test]
    fn csv_without_a_header_names_the_columns_by_position() {
        let table = parse(
            "1,alpha\n2,beta\n",
            &ImportOptions {
                format: ExportFormat::Csv,
                has_header: false,
                null_text: NULL_TEXT.to_string(),
            },
        )
        .expect("parses");
        assert_eq!(
            table.columns,
            vec!["COLUMN_1".to_string(), "COLUMN_2".to_string()]
        );
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn an_empty_null_text_makes_an_empty_cell_null() {
        let table = parse(
            "A,B\n,x\n",
            &ImportOptions {
                format: ExportFormat::Csv,
                has_header: true,
                null_text: String::new(),
            },
        )
        .expect("parses");
        assert_eq!(table.rows, vec![vec![None, Some("x".to_string())]]);
    }

    #[test]
    fn a_csv_null_text_only_applies_to_an_exact_match() {
        let table =
            parse("A,B,C\nNULL,null, NULL \n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                None,
                Some("null".to_string()),
                Some(" NULL ".to_string())
            ]]
        );
    }

    #[test]
    fn a_csv_header_cell_is_never_read_as_null() {
        let table = parse("NULL,B\n1,2\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.columns, vec!["NULL".to_string(), "B".to_string()]);
    }

    #[test]
    fn csv_records_end_at_every_line_ending() {
        for ending in ["\n", "\r\n", "\r"] {
            let text = format!("A,B{ending}1,2{ending}3,4{ending}");
            let table = parse(&text, &options(ExportFormat::Csv)).expect("parses");
            assert_eq!(table.rows.len(), 2, "{ending:?}");
        }
    }

    #[test]
    fn a_short_row_is_padded_with_nulls() {
        let table = parse("A,B,C\n1\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("1".to_string()), None, None]]);
    }

    #[test]
    fn a_row_wider_than_the_header_is_refused() {
        assert!(parse("A,B\n1,2,3\n", &options(ExportFormat::Csv)).is_err());
    }

    #[test]
    fn an_unnamed_column_is_refused() {
        assert!(parse("A,,C\n1,2,3\n", &options(ExportFormat::Csv)).is_err());
    }

    #[test]
    fn xml_tells_an_empty_element_from_an_empty_string() {
        let xml = "<results><row><A/><B></B></row></results>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows, vec![vec![None, Some(String::new())]]);
    }

    #[test]
    fn xml_fills_a_missing_element_with_null() {
        let xml = "<results><row><A>1</A><B>2</B></row><row><A>3</A></row></results>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows[1], vec![Some("3".to_string()), None]);
    }

    #[test]
    fn xml_reads_cdata_and_numeric_references() {
        let xml = "<r><row><A><![CDATA[a<b]]></A><B>&#13;&#x41;&amp;</B></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("a<b".to_string()), Some("\rA&".to_string())]]
        );
    }

    #[test]
    fn html_reads_a_table_that_uses_only_data_cells() {
        let html = "<table><tr><td>A</td><td>B</td></tr><tr><td>1</td><td>2</td></tr></table>";
        let table = parse(html, &options(ExportFormat::Html)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string()), Some("2".to_string())]]
        );
    }

    #[test]
    fn html_header_cells_win_over_the_header_choice() {
        let html =
            "<table><thead><tr><th>A</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>";
        let table = parse(
            html,
            &ImportOptions {
                format: ExportFormat::Html,
                has_header: false,
                null_text: NULL_TEXT.to_string(),
            },
        )
        .expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn html_ignores_the_document_around_the_table() {
        let html = "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
                    <style>td { content: \"<b>\"; }</style></head>\
                    <body><table><tr><th>A</th></tr><tr><td>1</td></tr></table></body></html>";
        let table = parse(html, &options(ExportFormat::Html)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn markdown_ignores_the_alignment_row() {
        let markdown = "| A | B |\n| :--- | ---: |\n| 1 | 2 |\n";
        let table = parse(markdown, &options(ExportFormat::Markdown)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string()), Some("2".to_string())]]
        );
    }

    #[test]
    fn detect_format_reads_the_extension() {
        for (name, format) in [
            ("rows.csv", ExportFormat::Csv),
            ("rows.TSV", ExportFormat::Tsv),
            ("rows.json", ExportFormat::Json),
            ("rows.xml", ExportFormat::Xml),
            ("rows.html", ExportFormat::Html),
            ("rows.htm", ExportFormat::Html),
            ("rows.md", ExportFormat::Markdown),
            ("rows.sql", ExportFormat::SqlInserts),
            ("rows.txt", ExportFormat::Csv),
        ] {
            assert_eq!(detect_format(Path::new(name)), Some(format), "{name}");
        }
        assert_eq!(detect_format(Path::new("rows.bin")), None);
    }

    #[test]
    fn only_the_formats_that_need_an_answer_ask_for_one() {
        for format in [ExportFormat::Csv, ExportFormat::Tsv] {
            assert!(header_choice_applies(format), "{}", format.label());
            assert!(null_text_choice_applies(format), "{}", format.label());
        }
        for format in [
            ExportFormat::Json,
            ExportFormat::Xml,
            ExportFormat::Markdown,
            ExportFormat::SqlInserts,
        ] {
            assert!(!header_choice_applies(format), "{}", format.label());
            assert!(!null_text_choice_applies(format), "{}", format.label());
        }
        // HTML can be written without header cells, but it never needs a NULL
        // text: an empty cell is the only way it spells NULL.
        assert!(header_choice_applies(ExportFormat::Html));
        assert!(!null_text_choice_applies(ExportFormat::Html));
    }

    #[test]
    fn an_empty_file_is_refused_for_every_format() {
        for format in ExportFormat::ALL {
            assert!(parse("", &options(format)).is_err(), "{}", format.label());
        }
    }

    #[test]
    fn invalid_json_reports_the_parser_error() {
        let error = parse("[{", &options(ExportFormat::Json)).expect_err("refused");
        assert!(error.starts_with("The JSON is not valid"), "{error}");
    }

    #[test]
    fn json_keeps_a_nested_value_verbatim() {
        let json = "[{\"A\": {\"b\": 1}, \"B\": [1, 2]}]";
        let table = parse(json, &options(ExportFormat::Json)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("{\"b\": 1}".to_string()),
                Some("[1, 2]".to_string())
            ]]
        );
    }

    #[test]
    fn json_unions_the_keys_of_every_object() {
        let json = "[{\"A\": 1}, {\"B\": 2}]";
        let table = parse(json, &options(ExportFormat::Json)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            table.rows,
            vec![
                vec![Some("1".to_string()), None],
                vec![None, Some("2".to_string())]
            ]
        );
    }

    // -----------------------------------------------------------------------
    // Adversarial cases: shapes that a naive reader gets wrong
    // -----------------------------------------------------------------------

    /// The column names `verify_result_export` uses to torture the writers:
    /// a space, a `(*)`, a duplicate, a leading digit, and a blank one.
    fn hostile_columns() -> Vec<String> {
        ["ID", "FULL NAME", "COUNT(*)", "ID", "2024_TOTAL", "NOTE"]
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    }

    fn hostile_name_grid() -> ExportGrid {
        ExportGrid {
            columns: hostile_columns(),
            column_kinds: vec![
                SqlValueKind::Number,
                SqlValueKind::String,
                SqlValueKind::Number,
                SqlValueKind::Number,
                SqlValueKind::Number,
                SqlValueKind::String,
            ],
            rows: vec![vec![
                "1".to_string(),
                "comma, and \"quotes\"".to_string(),
                "00123".to_string(),
                "1".to_string(),
                "-12.5".to_string(),
                "line1\nline2".to_string(),
            ]],
            null_text: NULL_TEXT.to_string(),
        }
    }

    #[test]
    fn csv_carries_hostile_column_names_through_unchanged() {
        let table = parse(
            &render(ExportFormat::Csv, &hostile_name_grid()),
            &options(ExportFormat::Csv),
        )
        .expect("parses");
        assert_eq!(table.columns, hostile_columns());
        assert_eq!(table.rows[0][1], Some("comma, and \"quotes\"".to_string()));
        assert_eq!(table.rows[0][5], Some("line1\nline2".to_string()));
    }

    #[test]
    fn json_carries_hostile_column_names_through_unchanged() {
        let table = parse(
            &render(ExportFormat::Json, &hostile_name_grid()),
            &options(ExportFormat::Json),
        )
        .expect("parses");
        // A duplicate key collapses: JSON objects have one value per name.
        assert_eq!(
            table.columns,
            vec![
                "ID".to_string(),
                "FULL NAME".to_string(),
                "COUNT(*)".to_string(),
                "2024_TOTAL".to_string(),
                "NOTE".to_string(),
            ]
        );
    }

    #[test]
    fn xml_reports_the_element_names_the_export_had_to_invent() {
        // XML cannot carry `COUNT(*)` or a name starting with a digit, so the
        // export rewrote them, and that is what comes back. The duplicate `ID`
        // collapses because an element name is looked up by name.
        let table = parse(
            &render(ExportFormat::Xml, &hostile_name_grid()),
            &options(ExportFormat::Xml),
        )
        .expect("parses");
        assert_eq!(
            table.columns,
            vec![
                "ID".to_string(),
                "FULL_NAME".to_string(),
                "COUNT___".to_string(),
                "column_5".to_string(),
                "NOTE".to_string(),
            ]
        );
        assert_eq!(table.rows[0][2], Some("00123".to_string()));
        assert_eq!(table.rows[0][4], Some("line1\nline2".to_string()));
    }

    #[test]
    fn a_number_keeps_its_exact_spelling_through_json() {
        // `serde_json::Value` would turn this into `12000000000.0`.
        let grid = ExportGrid {
            columns: vec!["N".to_string(), "M".to_string()],
            column_kinds: vec![SqlValueKind::Number, SqlValueKind::Number],
            rows: vec![vec!["1.2E+10".to_string(), "-0.000001".to_string()]],
            null_text: NULL_TEXT.to_string(),
        };
        let table = parse(
            &render(ExportFormat::Json, &grid),
            &options(ExportFormat::Json),
        )
        .expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("1.2E+10".to_string()),
                Some("-0.000001".to_string())
            ]]
        );
    }

    #[test]
    fn a_quote_inside_an_unquoted_csv_field_is_literal() {
        let table = parse("A,B\nab\"cd,x\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("ab\"cd".to_string()), Some("x".to_string())]]
        );
    }

    #[test]
    fn csv_keeps_text_that_follows_a_closing_quote() {
        let table = parse("A\n\"ab\"cd\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("abcd".to_string())]]);
    }

    #[test]
    fn a_quoted_csv_field_keeps_its_separators_and_line_endings() {
        let table = parse(
            "A,B\n\"x,y\r\nz\",\"tab\there\"\n",
            &options(ExportFormat::Csv),
        )
        .expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("x,y\r\nz".to_string()),
                Some("tab\there".to_string())
            ]]
        );
    }

    #[test]
    fn a_trailing_separator_makes_a_real_empty_last_field() {
        let table = parse("A,B,C\n1,2,\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("1".to_string()),
                Some("2".to_string()),
                Some(String::new())
            ]]
        );
    }

    #[test]
    fn a_last_record_without_a_line_ending_still_counts() {
        let table = parse("A,B\n1,2", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn an_unterminated_csv_quote_takes_the_rest_of_the_file() {
        let table = parse("A\n\"never closed\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("never closed\n".to_string())]]);
    }

    #[test]
    fn a_tsv_value_may_hold_commas() {
        let table = parse("A\tB\nx,y\tz\n", &options(ExportFormat::Tsv)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("x,y".to_string()), Some("z".to_string())]]
        );
    }

    #[test]
    fn xml_reads_text_split_around_a_nested_element() {
        let xml = "<r><row><A>before<b>mid</b>after</A></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("beforemidafter".to_string())]]);
    }

    #[test]
    fn xml_ignores_attributes_even_when_they_hold_markup() {
        let xml = "<r><row><A note=\"a &gt; b\" other='x>y'>v</A></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("v".to_string())]]);
    }

    #[test]
    fn xml_treats_a_self_closing_element_with_attributes_as_null() {
        let xml = "<r><row><A xsi:nil=\"true\"/><B>1</B></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows, vec![vec![None, Some("1".to_string())]]);
    }

    #[test]
    fn xml_keeps_a_namespace_prefix_as_part_of_the_column_name() {
        let xml = "<r><row><ns:A>1</ns:A></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.columns, vec!["ns:A".to_string()]);
    }

    #[test]
    fn xml_skips_comments_and_the_declaration() {
        let xml = "<?xml version=\"1.0\"?><!-- <row><A>ignored</A></row> -->\
                   <r><row><A>1</A></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn xml_takes_the_first_of_two_elements_with_the_same_name() {
        let xml = "<r><row><A>1</A><A>2</A></row></r>";
        let table = parse(xml, &options(ExportFormat::Xml)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn html_closes_a_cell_when_the_next_one_opens() {
        let html = "<TABLE><TR><TH>A<TH>B</TR><TR><TD>1<TD>2</TR></TABLE>";
        let table = parse(html, &options(ExportFormat::Html)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string()), Some("2".to_string())]]
        );
    }

    #[test]
    fn html_turns_a_line_break_element_into_a_newline() {
        let html = "<table><tr><th>A</th></tr><tr><td>one<br>two</td></tr></table>";
        let table = parse(html, &options(ExportFormat::Html)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("one\ntwo".to_string())]]);
    }

    #[test]
    fn html_reads_the_first_table_and_stops_at_it() {
        let html = "<table><tr><th>A</th></tr><tr><td>1</td></tr></table>\
                    <table><tr><th>B</th></tr><tr><td>2</td></tr></table>";
        let table = parse(html, &options(ExportFormat::Html)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn markdown_keeps_a_row_of_dashes_that_is_data() {
        let markdown = "| A |\n| --- |\n| --- |\n| x |\n";
        let table = parse(markdown, &options(ExportFormat::Markdown)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("---".to_string())], vec![Some("x".to_string())]]
        );
    }

    #[test]
    fn markdown_unescapes_a_pipe_and_a_backslash() {
        // What `escape_markdown_cell` writes for `a|b\c`.
        let markdown = "| A |\n| --- |\n| a\\|b\\\\c |\n";
        let table = parse(markdown, &options(ExportFormat::Markdown)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("a|b\\c".to_string())]]);
    }

    #[test]
    fn markdown_ignores_prose_between_the_rows() {
        let markdown = "some text\n| A |\n| --- |\n| 1 |\nmore text\n| 2 |\n";
        let table = parse(markdown, &options(ExportFormat::Markdown)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string())], vec![Some("2".to_string())]]
        );
    }

    #[test]
    fn sql_reads_a_string_holding_every_delimiter() {
        let sql = "INSERT INTO T (A) VALUES ('a)b,c;d--e/*f*/g');";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("a)b,c;d--e/*f*/g".to_string())]]);
    }

    #[test]
    fn sql_reads_a_statement_split_over_many_lines() {
        let sql = "insert into schema.t\n  (a,\n   b)\nvalues\n  (1,\n   'two')\n";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.columns, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string()), Some("two".to_string())]]
        );
    }

    #[test]
    fn sql_is_not_fooled_by_a_parenthesis_inside_a_quoted_table_name() {
        let sql = "INSERT INTO \"SC(H)EMA\".\"T\" (\"A\") VALUES (1);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn sql_unquotes_a_doubled_quote_inside_an_identifier() {
        let sql = "INSERT INTO T (\"A\"\"B\", `c``d`) VALUES (1, 2);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.columns, vec!["A\"B".to_string(), "c`d".to_string()]);
    }

    #[test]
    fn sql_without_a_column_list_says_so() {
        let error = parse(
            "INSERT INTO T VALUES (1, 2);",
            &options(ExportFormat::SqlInserts),
        )
        .expect_err("refused");
        assert!(error.contains("no column list"), "{error}");
    }

    #[test]
    fn sql_refuses_a_value_count_that_does_not_match_the_columns() {
        let error = parse(
            "INSERT INTO T (A, B) VALUES (1);",
            &options(ExportFormat::SqlInserts),
        )
        .expect_err("refused");
        assert!(error.contains("2 columns but 1 values"), "{error}");
    }

    #[test]
    fn sql_reads_signed_and_exponent_numbers() {
        let sql = "INSERT INTO T (A, B, C, D) VALUES (-1, +2, 1.5e-3, .5);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![
                Some("-1".to_string()),
                Some("+2".to_string()),
                Some("1.5e-3".to_string()),
                Some(".5".to_string()),
            ]]
        );
    }

    #[test]
    fn sql_reads_the_bare_booleans_a_boolean_column_exports() {
        let sql = "INSERT INTO T (A, B) VALUES (TRUE, false);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("TRUE".to_string()), Some("false".to_string())]]
        );
    }

    #[test]
    fn sql_reads_null_in_any_case() {
        let sql = "INSERT INTO T (A, B) VALUES (null, NuLl);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.rows, vec![vec![None, None]]);
    }

    #[test]
    fn sql_skips_a_block_comment_that_holds_a_statement() {
        let sql = "/* INSERT INTO X (A) VALUES (9); */ INSERT INTO T (A) VALUES (1);";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("1".to_string())]]);
    }

    #[test]
    fn sql_reads_a_final_statement_without_a_semicolon() {
        let sql = "INSERT INTO T (A) VALUES (1);\nINSERT INTO T (A) VALUES (2)";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn a_mysql_backslash_escape_survives_a_round_trip_into_oracle() {
        // The file says which dialect wrote it by how it quotes columns, so a
        // MySQL export reads back exactly even though the target is Oracle.
        let mysql = build_sql_inserts(&selection(DatabaseType::MySQL));
        let oracle = build_sql_inserts(&selection(DatabaseType::Oracle));
        assert!(mysql.contains('`') && !oracle.contains('`'));
        let from_mysql = parse(&mysql, &options(ExportFormat::SqlInserts)).expect("parses");
        let from_oracle = parse(&oracle, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(from_mysql, from_oracle);
    }

    #[test]
    fn every_format_survives_a_grid_with_no_rows() {
        let empty = ExportGrid {
            columns: vec!["A".to_string(), "B".to_string()],
            column_kinds: vec![SqlValueKind::Number, SqlValueKind::String],
            rows: Vec::new(),
            null_text: NULL_TEXT.to_string(),
        };
        for format in ExportFormat::ALL {
            if format == ExportFormat::SqlInserts {
                // No rows means no INSERT statements, so there is no file.
                continue;
            }
            let text = render(format, &empty);
            let table = match parse(&text, &options(format)) {
                Ok(table) => table,
                // JSON and XML write nothing a column list can be read from.
                Err(_) if matches!(format, ExportFormat::Json | ExportFormat::Xml) => continue,
                Err(error) => panic!("{} failed: {error}", format.label()),
            };
            assert_eq!(table.columns.len(), 2, "{}", format.label());
            assert!(table.rows.is_empty(), "{}", format.label());
        }
    }

    #[test]
    fn a_thousand_rows_round_trip_through_every_format() {
        let rows: Vec<Vec<String>> = (0..1000)
            .map(|index| {
                vec![
                    index.to_string(),
                    format!("name |{index}, \"q\"\n{index}"),
                    if index % 7 == 0 {
                        NULL_TEXT.to_string()
                    } else {
                        format!("{index:08}")
                    },
                ]
            })
            .collect();
        let grid = ExportGrid {
            columns: vec!["ID".to_string(), "NAME".to_string(), "CODE".to_string()],
            column_kinds: vec![
                SqlValueKind::Number,
                SqlValueKind::String,
                SqlValueKind::String,
            ],
            rows,
            null_text: NULL_TEXT.to_string(),
        };
        for format in ExportFormat::ALL {
            let text = if format == ExportFormat::SqlInserts {
                build_sql_inserts(&GridSqlSelection {
                    db_type: DatabaseType::Oracle,
                    table: Some("T".to_string()),
                    all_columns: grid.columns.clone(),
                    column_kinds: grid.column_kinds.clone(),
                    selected_columns: (0..grid.columns.len()).collect(),
                    rows: grid.rows.clone(),
                    null_text: grid.null_text.clone(),
                })
            } else {
                render(format, &grid)
            };
            let table = parse(&text, &options(format))
                .unwrap_or_else(|error| panic!("{} failed: {error}", format.label()));
            assert_eq!(table.rows.len(), 1000, "{}", format.label());
            assert_eq!(table.rows[7][2], None, "{}", format.label());
            assert_eq!(
                table.rows[1][2],
                Some("00000001".to_string()),
                "{}",
                format.label()
            );
        }
    }
}
