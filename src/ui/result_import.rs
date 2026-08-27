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
        // Extensions no format claims outright. `.text` sits beside `.txt`
        // rather than beside `.markdown`: both name plain text, and having one
        // preselect Markdown while the other preselected CSV was a difference
        // with no reason behind it.
        .or(match extension.as_str() {
            "txt" | "text" => Some(ExportFormat::Csv),
            "markdown" => Some(ExportFormat::Markdown),
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

/// One field of a delimited record, and whether the file QUOTED it.
///
/// The quoting is data, not decoration. It is the only thing a delimited file
/// has left to tell SQL NULL from a value that spells the NULL text the same
/// way — the writer quotes the value and leaves the NULL bare
/// ([`crate::ui::result_export::ExportGrid::display_cell`]) — and dropping it
/// at the door is what made this app's own export → import turn a `VARCHAR`
/// holding the four letters `NULL` into a real NULL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DelimitedField {
    text: String,
    quoted: bool,
}

/// Read RFC 4180 records: quoted fields may hold the separator, a line break,
/// or a doubled quote. Records end at LF, CRLF, or a lone CR.
///
/// A BLANK LINE is not a record. It used to be one field of empty text, which
/// `validate` then padded out to the file's full width — so a file with a
/// trailing empty line imported one extra row of NULLs, or lost the whole
/// import to a NOT NULL column. Every other reader agrees: Python's `csv`
/// yields nothing for it and pandas skips it. A line that really does hold one
/// empty value writes it as `""`, which is quoted and therefore still a record.
///
/// The honest limit: with the NULL text set to empty, a row of ONE column
/// holding NULL is written as an empty line, and an empty line is what a blank
/// one is. Nothing in the file separates them, so such a row reads as absent.
/// Every other shape — a second column, or any non-empty NULL text — carries
/// enough to say which it is.
fn split_delimited_records(text: &str, separator: char) -> Vec<Vec<DelimitedField>> {
    let mut records: Vec<Vec<DelimitedField>> = Vec::new();
    let mut record: Vec<DelimitedField> = Vec::new();
    let mut field = String::new();
    let mut field_quoted = false;
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
                field_quoted = true;
                in_quotes = true;
            }
            _ if ch == separator => {
                record.push(DelimitedField {
                    text: std::mem::take(&mut field),
                    quoted: std::mem::take(&mut field_quoted),
                });
                field_started = false;
                record_started = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                // Nothing started this line — no character, no quote, no
                // separator — so there is no record here to end. Anything that
                // could have filled `field` or `record` sets `record_started`
                // in the same breath, which is why this one flag answers it.
                if record_started {
                    record.push(DelimitedField {
                        text: std::mem::take(&mut field),
                        quoted: std::mem::take(&mut field_quoted),
                    });
                    records.push(std::mem::take(&mut record));
                }
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
    if record_started {
        record.push(DelimitedField {
            text: field,
            quoted: field_quoted,
        });
        records.push(record);
    }
    records
}

/// Whether quoting can tell a NULL from a value that spells the NULL text.
///
/// The writer leaves a NULL bare and quotes the value, so the two are only
/// distinguishable while the NULL text needs no quotes of its own. One that
/// holds the separator, a quote, or a line break must be quoted whichever it
/// means, and then the signal is spent — the reader falls back to matching the
/// text, exactly as it did before the signal existed.
///
/// Asked of [`crate::ui::result_export::delimited_field_needs_quotes`], the
/// writer's own rule, so the two sides cannot come to disagree about which
/// texts carry the signal.
fn null_text_quoting_is_a_signal(null_text: &str, separator: char) -> bool {
    !crate::ui::result_export::delimited_field_needs_quotes(null_text, separator)
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
    let quoting_is_a_signal = null_text_quoting_is_a_signal(&options.null_text, separator);
    let to_cells = |record: Vec<DelimitedField>| -> Vec<ImportCell> {
        record
            .into_iter()
            .map(|field| {
                // The writer's exact inverse: a NULL is the NULL text written
                // bare, and the same text QUOTED is a value that spells it.
                // Where the NULL text needs quotes of its own the signal does
                // not exist, and the text alone decides — which is what this
                // has always done, so no file loses a NULL it used to keep.
                let is_null =
                    field.text == options.null_text && (!field.quoted || !quoting_is_a_signal);
                if is_null {
                    None
                } else {
                    Some(field.text)
                }
            })
            .collect()
    };

    // The header line is text even when it happens to equal the NULL text.
    let first_cells: Vec<ImportCell> = if options.has_header {
        first.into_iter().map(|field| Some(field.text)).collect()
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
                    // The LAST entry wins when one object repeats a key, which
                    // is what `serde_json` itself does and what a reader that
                    // built a map would end up with. Taking the first meant this
                    // reader and the rest of the app disagreed about the same
                    // document.
                    record
                        .0
                        .iter()
                        .rev()
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
    /// The tag's text after its name, kept raw. Only the HTML reader looks at
    /// it, and only for `colspan`/`rowspan`: a cell that spans is the one place
    /// where an attribute decides which COLUMN a value belongs to.
    attributes: String,
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

    /// A positive integer attribute, clamped to what HTML itself allows.
    ///
    /// A missing, unreadable, or zero value means "one", which is the span a
    /// cell has when it says nothing. The clamp is the spec's own: without it a
    /// `colspan="99999999"` in a file would be an allocation this reader made on
    /// the file's say-so.
    fn span_attribute(&self, name: &str, limit: usize) -> usize {
        attribute_value(&self.attributes, name)
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, limit)
    }
}

/// The value of `name` in a tag's attribute text, quoted or bare.
fn attribute_value(attributes: &str, name: &str) -> Option<String> {
    let lowered = attributes.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(offset) = lowered[from..].find(name) {
        let at = from + offset;
        from = at + name.len();
        // A whole attribute name, not the tail of another one.
        if at > 0 && {
            let before = lowered.as_bytes()[at - 1];
            before.is_ascii_alphanumeric() || before == b'-' || before == b'_'
        } {
            continue;
        }
        let rest = lowered[from..].trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        let value_at = attributes.len() - rest.len() + 1;
        let value = attributes[value_at..].trim_start();
        let quoted = value
            .strip_prefix('"')
            .and_then(|inner| inner.split('"').next())
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|inner| inner.split('\'').next())
            });
        return Some(match quoted {
            Some(inner) => inner.to_string(),
            None => value
                .split(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
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
            // A `<table>` opens a scope these rules do not reach across: a row
            // of an INNER table is not a sibling of a row of the outer one, and
            // treating it as one closed the outer row and promoted the inner
            // one beside it.
            let floor = stack
                .iter()
                .rposition(|node| node.is("table"))
                .map_or(0, |at| at + 1);
            if let Some(position) = stack[floor..]
                .iter()
                .rposition(|node| closes.iter().any(|tag| node.is(tag)))
            {
                close_to(&mut stack, &mut roots, floor + position);
            }
        }
        let node = MarkupNode {
            self_closing,
            attributes: tag_attributes(&inner),
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

/// Everything in a tag after its name, with the self-closing slash dropped.
fn tag_attributes(inner: &str) -> String {
    let inner = inner.trim_start().trim_start_matches('/');
    let name_len = inner
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != '/' && *ch != '>')
        .map(char::len_utf8)
        .sum::<usize>();
    inner[name_len..].trim().trim_end_matches('/').to_string()
}

/// The longest entity body this resolves: `#x10FFFF` is eight characters, and
/// ten leaves room for a named one no shorter than the numeric forms.
const MAX_ENTITY_BODY: usize = 10;

/// The byte offset of the `;` that closes the entity starting at `text[0] == '&'`,
/// or `None` when there is none within reach.
///
/// The bound gates the SEARCH, not the result. It used to be a filter applied
/// after `find(';')` had already scanned the whole remaining text — so text that
/// is dense in `&` and holds no `;` made every one of them scan to the end of
/// the file, which is quadratic. Measured on an XML import: 40 KB of bare `&`
/// took 50 ms, 80 KB took 131 ms, 160 KB took 517 ms and 320 KB took 2069 ms —
/// a clean ×4 per doubling, extrapolating to minutes for a few megabytes, on the
/// UI thread inside the import dialog. The same byte count written as `&amp;` —
/// five times the text — is linear and finishes in 81 ms, because each `;` is
/// found immediately.
///
/// A `;` at byte `MAX_ENTITY_BODY` is preceded by at most that many bytes and so
/// by at most that many characters, which is why looking at one character more
/// than the bound is enough to find every terminator the old filter accepted.
fn entity_terminator(text: &str) -> Option<usize> {
    text.char_indices()
        .take(MAX_ENTITY_BODY + 1)
        .find(|(_, ch)| *ch == ';')
        .map(|(index, _)| index)
        .filter(|end| *end <= MAX_ENTITY_BODY)
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
        let Some(end) = entity_terminator(tail) else {
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
    collect_html_rows(table, &mut HtmlRowBuilder::default(), &mut rows);
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

/// HTML's own ceilings on how far one cell may span. Applied so a number in a
/// file cannot ask this reader for an arbitrary amount of memory.
const MAX_COLSPAN: usize = 1000;
const MAX_ROWSPAN: usize = 65534;

/// Turns `<tr>` elements into rows that all address the same columns.
///
/// A cell is not a column: `colspan` makes one cell fill several, and `rowspan`
/// makes it re-appear in the rows below. Reading one cell as one column shifted
/// every later cell of a spanned row one place to the left and left the rows
/// under a `rowspan` short — silently, into the wrong column of the target
/// table. Owning both rules in one place is what keeps a row's width a property
/// of the TABLE rather than of the row that happened to be read last.
#[derive(Default)]
struct HtmlRowBuilder {
    /// Per column, a value a `rowspan` still owes to later rows, and how many.
    carry: Vec<Option<(usize, ImportCell)>>,
}

impl HtmlRowBuilder {
    /// One `<tr>` as cells, or `None` when it contributes nothing at all.
    /// The flag says whether the row was written with `<th>` cells.
    ///
    /// A `<tr>` with no cells of its own is still a row when a `rowspan` above
    /// it owes this row a value: skipping it outright would hand that value to
    /// the NEXT row instead, one row too early.
    fn row(&mut self, tr: &MarkupNode) -> Option<(bool, Vec<ImportCell>)> {
        let mut header = false;
        let mut cells = tr
            .elements()
            .filter(|cell| cell.is("td") || cell.is("th"))
            .inspect(|cell| header |= cell.is("th"))
            .peekable();
        let has_cells = cells.peek().is_some();

        let mut out: Vec<ImportCell> = Vec::new();
        let mut column = 0usize;
        loop {
            if let Some(value) = self.take_carried(column) {
                out.push(value);
                column += 1;
                continue;
            }
            let Some(cell) = cells.next() else { break };
            let text = cell.all_text();
            // The exporter writes NULL as an empty cell.
            let value = (!text.is_empty()).then_some(text);
            let colspan = cell.span_attribute("colspan", MAX_COLSPAN);
            let rowspan = cell.span_attribute("rowspan", MAX_ROWSPAN);
            for _ in 0..colspan {
                if self.carry.len() <= column {
                    self.carry.resize(column + 1, None);
                }
                out.push(value.clone());
                self.carry[column] = (rowspan > 1).then(|| (rowspan - 1, value.clone()));
                column += 1;
            }
        }
        // Columns past the last cell that a `rowspan` still owes this row.
        while column < self.carry.len() {
            let Some(value) = self.take_carried(column) else {
                break;
            };
            out.push(value);
            column += 1;
        }
        (has_cells || !out.is_empty()).then_some((header, out))
    }

    /// The value a `rowspan` owes `column` in the row being built, if any.
    fn take_carried(&mut self, column: usize) -> Option<ImportCell> {
        let slot = self.carry.get_mut(column)?;
        let (remaining, value) = slot.as_mut()?;
        let value = value.clone();
        *remaining -= 1;
        if *remaining == 0 {
            *slot = None;
        }
        Some(value)
    }
}

/// Gather `<tr>` rows from a table, descending through `<thead>`/`<tbody>`.
fn collect_html_rows(
    node: &MarkupNode,
    builder: &mut HtmlRowBuilder,
    rows: &mut Vec<(bool, Vec<ImportCell>)>,
) {
    for child in node.elements() {
        if child.is("tr") {
            if let Some(row) = builder.row(child) {
                rows.push(row);
            }
        } else if !child.is("table") {
            collect_html_rows(child, builder, rows);
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
///
/// One scan, in the reverse order the writer applied its rules: `\\`, `\|` and
/// `\<` give back the character that follows, and only a `<br>` whose `<` was
/// NOT escaped is the line break the writer inserted. Substituting `<br>` in a
/// separate pass afterwards — as this did — turned a `<br>` that was in the data
/// into a newline, because by then the two were spelled the same.
fn split_markdown_row(line: &str) -> Vec<ImportCell> {
    let line = line.trim();
    let inner = line
        .strip_prefix('|')
        .unwrap_or(line)
        .strip_suffix('|')
        .unwrap_or_else(|| line.strip_prefix('|').unwrap_or(line));

    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut index = 0usize;
    while index < inner.len() {
        let rest = &inner[index..];
        let Some(ch) = rest.chars().next() else { break };
        if ch == '\\' {
            index += ch.len_utf8();
            match inner[index..].chars().next() {
                // What the exporter writes for a literal backslash, pipe or
                // less-than sign.
                Some(next @ ('\\' | '|' | '<')) => {
                    cell.push(next);
                    index += next.len_utf8();
                }
                // Any other backslash is itself, so a hand-written file keeps
                // its text.
                Some(next) => {
                    cell.push('\\');
                    cell.push(next);
                    index += next.len_utf8();
                }
                None => cell.push('\\'),
            }
            continue;
        }
        if ch == '|' {
            cells.push(std::mem::take(&mut cell));
            index += ch.len_utf8();
            continue;
        }
        if rest.starts_with(MARKDOWN_LINE_BREAK) {
            cell.push('\n');
            index += MARKDOWN_LINE_BREAK.len();
            continue;
        }
        cell.push(ch);
        index += ch.len_utf8();
    }
    cells.push(cell);

    cells
        .into_iter()
        .map(|cell| {
            let cell = cell.trim().to_string();
            // The exporter writes NULL as an empty cell.
            (!cell.is_empty()).then_some(cell)
        })
        .collect()
}

/// The markup a Markdown cell spells a line break with — the one piece of it
/// that is structure rather than data, which is why every other `<` is escaped.
const MARKDOWN_LINE_BREAK: &str = "<br>";

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
    // One file, one writer, one answer — asked before the first split, because
    // the split itself depends on it.
    let dialect = detect_sql_file_dialect(text);
    let statements = split_sql_statements(text, dialect);
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<ImportCell>> = Vec::new();

    for statement in statements {
        let Some(insert) = split_insert_statement(&statement, dialect)? else {
            continue;
        };
        if columns.is_empty() {
            columns = insert.columns.clone();
        }
        // The same columns in another ORDER name the same row; only a
        // different SET of columns is a file this cannot read as one table.
        let order = column_order_against(&columns, &insert.columns).ok_or_else(|| {
            "The INSERT statements do not all use the same column list.".to_string()
        })?;
        for values in &insert.rows {
            if values.len() != insert.columns.len() {
                return Err(format!(
                    "An INSERT lists {} columns but {} values.",
                    insert.columns.len(),
                    values.len()
                ));
            }
            rows.push(
                order
                    .iter()
                    .map(|source| sql_value_text(&values[*source], dialect))
                    .collect::<Result<Vec<_>, String>>()?,
            );
        }
    }

    if columns.is_empty() {
        return Err("The file has no INSERT statement with a column list.".to_string());
    }
    Ok(ImportedTable { columns, rows })
}

/// Which escape rules a file's string literals follow.
///
/// The two dialects disagree about ONE thing, and it is the thing that decides
/// where a literal ENDS: MySQL and MariaDB read `\` as an escape, Oracle reads
/// it as an ordinary character. `'C:\path\'` is therefore a complete Oracle
/// literal and an unterminated MySQL one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlFileDialect {
    Oracle,
    MysqlFamily,
}

impl SqlFileDialect {
    fn backslash_escapes(self) -> bool {
        matches!(self, SqlFileDialect::MysqlFamily)
    }
}

/// Which dialect a file of `INSERT` statements is written in.
///
/// Decided ONCE, for the whole file, and BEFORE anything is split. It used to be
/// decided per statement, from the backticks in that statement's column list —
/// which is only readable after the file has already been split, so the splitter
/// ran with one answer and the value decoder with another. An Oracle file
/// holding `'C:\path\'` then lost every statement after it, and one holding
/// `'a\''b'` decoded to `a\`. One file has one writer, so one answer.
///
/// The signal is a backtick-quoted identifier: only the MySQL family writes one,
/// and this app's `SQL Inserts` export always writes one for that family. The
/// pre-pass scans with Oracle's rule because that rule ends BOTH dialects'
/// literals correctly for the forms this app writes — its MySQL literals double
/// every backslash and spell a quote `''`, so an odd run of backslashes can
/// never precede a closing quote.
fn detect_sql_file_dialect(text: &str) -> SqlFileDialect {
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let Some(ch) = rest.chars().next() else { break };
        match ch {
            '\'' => {
                index = string_literal_end(text, index, SqlFileDialect::Oracle);
                continue;
            }
            '"' => {
                index = quoted_identifier_end(text, index, '"');
                continue;
            }
            '`' => return SqlFileDialect::MysqlFamily,
            '-' if rest.starts_with("--") => {
                index = rest.find('\n').map_or(text.len(), |at| index + at + 1);
                continue;
            }
            '/' if rest.starts_with("/*") => {
                index = block_comment_end(text, index);
                continue;
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    SqlFileDialect::Oracle
}

/// The byte index just past the string literal whose opening `'` is at `open`.
///
/// The ONE place a literal's end is decided. Statement splitting, paren
/// matching, list splitting and value decoding all ask it, so none of them can
/// hold a different opinion about where a value stops — which is exactly the
/// disagreement that used to swallow statements.
///
/// An unterminated literal ends at end of input rather than reporting: this
/// reader is deliberately forgiving, and the caller reports the shape it could
/// not make sense of.
fn string_literal_end(text: &str, open: usize, dialect: SqlFileDialect) -> usize {
    let mut index = open + '\''.len_utf8();
    while index < text.len() {
        let rest = &text[index..];
        let Some(ch) = rest.chars().next() else { break };
        if ch == '\\' && dialect.backslash_escapes() {
            index += ch.len_utf8();
            if let Some(escaped) = text[index..].chars().next() {
                index += escaped.len_utf8();
            }
            continue;
        }
        index += ch.len_utf8();
        if ch == '\'' {
            // A doubled quote is one character of the value in every dialect.
            if text[index..].starts_with('\'') {
                index += '\''.len_utf8();
                continue;
            }
            return index;
        }
    }
    text.len()
}

/// The byte index just past the quoted identifier opened at `open`.
///
/// A doubled delimiter is one character of the NAME, so it does not close it —
/// `` `zr``tick` `` is one identifier, not two.
fn quoted_identifier_end(text: &str, open: usize, quote: char) -> usize {
    let mut index = open + quote.len_utf8();
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        index += ch.len_utf8();
        if ch == quote {
            if text[index..].starts_with(quote) {
                index += quote.len_utf8();
                continue;
            }
            return index;
        }
    }
    text.len()
}

/// The byte index just past the `/* … */` comment opened at `index`.
fn block_comment_end(text: &str, index: usize) -> usize {
    text[index + 2..]
        .find("*/")
        .map_or(text.len(), |at| index + 2 + at + 2)
}

/// Split on `;` outside string literals, comments, and parentheses.
///
/// Comments are dropped rather than carried into the statement, because what
/// follows reads the statement by its first word and by its first `(`.
fn split_sql_statements(text: &str, dialect: SqlFileDialect) -> Vec<String> {
    let mut statements: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut index = 0usize;

    while index < text.len() {
        let rest = &text[index..];
        let Some(ch) = rest.chars().next() else { break };
        match ch {
            '\'' => {
                let end = string_literal_end(text, index, dialect);
                current.push_str(&text[index..end]);
                index = end;
                continue;
            }
            '"' | '`' => {
                // A quoted identifier can hold a `;` or a paren just as a
                // string can.
                let end = quoted_identifier_end(text, index, ch);
                current.push_str(&text[index..end]);
                index = end;
                continue;
            }
            '-' if rest.starts_with("--") => {
                index = rest.find('\n').map_or(text.len(), |at| index + at + 1);
                current.push('\n');
                continue;
            }
            '/' if rest.starts_with("/*") => {
                index = block_comment_end(text, index);
                current.push(' ');
                continue;
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
        index += ch.len_utf8();
    }
    if !current.trim().is_empty() {
        statements.push(current);
    }
    statements
        .into_iter()
        .filter(|statement| !statement.trim().is_empty())
        .collect()
}

/// One INSERT statement's payload: the columns it names and EVERY row of values
/// it carries.
///
/// A statement carries MANY rows, not one. It used to be read as one — the first
/// `(…)` after `VALUES` and nothing else — so a multi-row `VALUES (…),(…)` and an
/// Oracle `INSERT ALL` both gave back their first row and dropped the rest with
/// no error at all. Both shapes are what this app's own import script builder
/// writes, and the multi-row `VALUES` list is what `mysqldump` writes.
struct InsertRows {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// The driving query an `INSERT ALL` of literal rows ends with.
///
/// It supplies exactly one row, which is what makes each `INTO` contribute
/// exactly one. Any other query means the statement inserts one row per row it
/// returns — a count this reader cannot know — so such a file is refused by name
/// rather than read as if the query returned one row.
const ORACLE_MULTI_INSERT_DRIVER: &str = "SELECT * FROM DUAL";

/// Pull the columns and every row of values out of one INSERT. `Ok(None)` means
/// the statement is not an INSERT and should be ignored.
fn split_insert_statement(
    statement: &str,
    dialect: SqlFileDialect,
) -> Result<Option<InsertRows>, String> {
    let trimmed = statement.trim();
    let mut words = trimmed.split_whitespace();
    if !words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("insert"))
    {
        return Ok(None);
    }
    match words.next() {
        // `INSERT FIRST` routes each row by its `WHEN` clauses, so which target
        // a row lands in is not a property of the row. Say so.
        Some(word) if word.eq_ignore_ascii_case("first") => Err(
            "An INSERT FIRST statement sends its rows to different targets, so this import \
             cannot read them as one table."
                .to_string(),
        ),
        Some(word) if word.eq_ignore_ascii_case("all") => {
            split_oracle_multi_insert(trimmed, dialect).map(Some)
        }
        _ => split_single_target_insert(trimmed, dialect).map(Some),
    }
}

/// `INSERT [INTO] t (a, b) VALUES (…), (…), …`
fn split_single_target_insert(
    trimmed: &str,
    dialect: SqlFileDialect,
) -> Result<InsertRows, String> {
    let Some(columns_open) = find_open_paren(trimmed, dialect) else {
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
    let columns_close = matching_paren(trimmed, columns_open, dialect)
        .ok_or_else(|| "An INSERT column list is not closed.".to_string())?;
    let columns = read_column_list(&trimmed[columns_open + 1..columns_close], dialect);

    let tail = &trimmed[columns_close + 1..];
    let values_at = find_keyword(tail, "values", dialect)
        .ok_or_else(|| "An INSERT statement has no VALUES list.".to_string())?;
    let (rows, _) = read_value_groups(tail, values_at + "values".len(), dialect)?;
    Ok(InsertRows { columns, rows })
}

/// `INSERT ALL INTO t (a) VALUES (…) INTO t (a) VALUES (…) SELECT * FROM DUAL`
fn split_oracle_multi_insert(trimmed: &str, dialect: SqlFileDialect) -> Result<InsertRows, String> {
    let mut columns: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut at = 0usize;

    while let Some(offset) = find_keyword(&trimmed[at..], "into", dialect) {
        let into_at = at + offset;
        let target = &trimmed[into_at..];
        let Some(columns_open) = find_open_paren(target, dialect) else {
            return Err("An INSERT ALL target has no column list.".to_string());
        };
        // The same reading as the single-target path: a `(` straight after
        // VALUES is the row, not the column list, so the target names no
        // columns to map its values onto.
        if target[..columns_open]
            .split_whitespace()
            .next_back()
            .is_some_and(|word| word.eq_ignore_ascii_case("values"))
        {
            return Err(
                "An INSERT ALL target has no column list, so its values cannot be mapped to \
                 columns."
                    .to_string(),
            );
        }
        let columns_close = matching_paren(target, columns_open, dialect)
            .ok_or_else(|| "An INSERT ALL column list is not closed.".to_string())?;
        let target_columns = read_column_list(&target[columns_open + 1..columns_close], dialect);
        // Every target keeps its values in ITS OWN column order, and this
        // statement yields ONE column list, so each target's rows are lifted
        // into the first target's order here.
        let first_columns = columns.get_or_insert_with(|| target_columns.clone());
        let order = column_order_against(first_columns, &target_columns).ok_or_else(|| {
            "The targets of one INSERT ALL do not all use the same column list.".to_string()
        })?;
        let values_at =
            find_keyword(&target[columns_close + 1..], "values", dialect).ok_or_else(|| {
                "An INSERT ALL target has no VALUES list, so it carries no row to read.".to_string()
            })?;
        let (target_rows, consumed) = read_value_groups(
            target,
            columns_close + 1 + values_at + "values".len(),
            dialect,
        )?;
        for values in target_rows {
            if values.len() != target_columns.len() {
                return Err(format!(
                    "An INSERT ALL target lists {} columns but {} values.",
                    target_columns.len(),
                    values.len()
                ));
            }
            rows.push(order.iter().map(|source| values[*source].clone()).collect());
        }
        at = into_at + consumed;
    }

    let columns = columns
        .ok_or_else(|| "An INSERT ALL statement names no target to read rows from.".to_string())?;
    // What follows the last target is the driving query, and only a one-row one
    // leaves each target contributing exactly one row.
    let driver = collapse_whitespace(&trimmed[at..]);
    if driver.is_empty() {
        return Err(
            "An INSERT ALL statement has no driving query, so it is not a statement this import \
             can read rows from."
                .to_string(),
        );
    }
    if !driver.eq_ignore_ascii_case(ORACLE_MULTI_INSERT_DRIVER) {
        return Err(format!(
            "An INSERT ALL is driven by a query this import cannot count rows for: {driver}"
        ));
    }
    Ok(InsertRows { columns, rows })
}

/// Read `(…), (…), …` starting at `from`, and how much of `text` was consumed.
///
/// Groups are separated by top-level commas. Anything else ends the list, so a
/// trailing clause the statement may carry — `ON DUPLICATE KEY UPDATE`,
/// `RETURNING`, the driving query of an `INSERT ALL` — stops it without being
/// read as a row. A comma that is NOT followed by a group is refused rather than
/// dropped: that is text this reader did not understand.
fn read_value_groups(
    text: &str,
    from: usize,
    dialect: SqlFileDialect,
) -> Result<(Vec<Vec<String>>, usize), String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut index = from;
    loop {
        let after_space = index + text[index..].len() - text[index..].trim_start().len();
        if !text[after_space..].starts_with('(') {
            if rows.is_empty() {
                return Err("An INSERT VALUES list is missing.".to_string());
            }
            return Ok((rows, index));
        }
        let close = matching_paren(text, after_space, dialect)
            .ok_or_else(|| "An INSERT VALUES list is not closed.".to_string())?;
        let group = &text[after_space + 1..close];
        if group.trim().is_empty() {
            return Err("An INSERT VALUES list holds no values.".to_string());
        }
        rows.push(
            split_top_level(group, dialect)
                .into_iter()
                .map(|value| value.trim().to_string())
                .collect(),
        );
        index = close + 1;

        let rest = text[index..].trim_start();
        if !rest.starts_with(',') {
            return Ok((rows, index));
        }
        let comma_at = index + text[index..].len() - rest.len();
        let after_comma = &text[comma_at + 1..];
        if !after_comma.trim_start().starts_with('(') {
            return Err(
                "An INSERT VALUES list has a comma that is not followed by another row."
                    .to_string(),
            );
        }
        index = comma_at + 1;
    }
}

fn read_column_list(text: &str, dialect: SqlFileDialect) -> Vec<String> {
    split_top_level(text, dialect)
        .into_iter()
        .map(|column| unquote_identifier(column.trim()))
        .collect()
}

/// How `columns` map onto `wanted`, or `None` when they are not the same set.
///
/// The same columns in another ORDER name the same row, and a file that says so
/// used to be refused outright. Names are matched the way SQL resolves an
/// unquoted identifier, which is also how the import dialog matches a file
/// column to a table column.
fn column_order_against(wanted: &[String], columns: &[String]) -> Option<Vec<usize>> {
    if wanted.len() != columns.len() {
        return None;
    }
    let mut taken = vec![false; columns.len()];
    let mut order = Vec::with_capacity(wanted.len());
    for name in wanted {
        let found = columns
            .iter()
            .enumerate()
            .position(|(index, candidate)| !taken[index] && candidate.eq_ignore_ascii_case(name))?;
        taken[found] = true;
        order.push(found);
    }
    Some(order)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `(` that is not inside a string literal or a quoted identifier —
/// a schema like `"SC(H)EMA"` must not be mistaken for the column list.
fn find_open_paren(text: &str, dialect: SqlFileDialect) -> Option<usize> {
    let mut index = 0usize;
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        match ch {
            '\'' => {
                index = string_literal_end(text, index, dialect);
                continue;
            }
            '"' | '`' => {
                index = quoted_identifier_end(text, index, ch);
                continue;
            }
            '(' => return Some(index),
            _ => {}
        }
        index += ch.len_utf8();
    }
    None
}

fn matching_paren(text: &str, open: usize, dialect: SqlFileDialect) -> Option<usize> {
    let mut depth = 0i32;
    let mut index = open;
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        match ch {
            '\'' => {
                index = string_literal_end(text, index, dialect);
                continue;
            }
            '"' | '`' => {
                index = quoted_identifier_end(text, index, ch);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    None
}

/// Split a comma-separated list, ignoring commas inside strings, quoted
/// identifiers, or nested parentheses.
fn split_top_level(text: &str, dialect: SqlFileDialect) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        match ch {
            '\'' => {
                index = string_literal_end(text, index, dialect);
                continue;
            }
            '"' | '`' => {
                index = quoted_identifier_end(text, index, ch);
                continue;
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    parts.push(&text[start..]);
    parts
}

/// Split on the top-level `||` an Oracle concatenation is written with.
fn split_concatenation(text: &str, dialect: SqlFileDialect) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let Some(ch) = rest.chars().next() else { break };
        match ch {
            '\'' => {
                index = string_literal_end(text, index, dialect);
                continue;
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 && rest.starts_with("||") => {
                parts.push(&text[start..index]);
                index += 2;
                start = index;
                continue;
            }
            _ => {}
        }
        index += ch.len_utf8();
    }
    parts.push(&text[start..]);
    parts
}

/// Find `keyword` as a whole word outside string literals and quoted
/// identifiers.
///
/// The scan walks the text through the same readers everything else here uses,
/// so a value or a column name that happens to spell the keyword cannot be
/// mistaken for it. `keyword` must be lower-case ASCII.
fn find_keyword(text: &str, keyword: &str, dialect: SqlFileDialect) -> Option<usize> {
    // Compared in place rather than against a lower-cased copy: this is called
    // once per target of an `INSERT ALL`, and a copy of the remaining text per
    // call would make reading one statement quadratic in its length.
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let ch = rest.chars().next()?;
        match ch {
            '\'' => {
                index = string_literal_end(text, index, dialect);
                continue;
            }
            '"' | '`' => {
                index = quoted_identifier_end(text, index, ch);
                continue;
            }
            _ => {}
        }
        if text
            .get(index..index + keyword.len())
            .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
        {
            let before_ok =
                index == 0 || !bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_';
            let after = index + keyword.len();
            let after_ok = after >= bytes.len()
                || !bytes[after].is_ascii_alphanumeric() && bytes[after] != b'_';
            if before_ok && after_ok {
                return Some(index);
            }
        }
        index += ch.len_utf8();
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
fn sql_value_text(value: &str, dialect: SqlFileDialect) -> Result<ImportCell, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    if let Some(text) = sql_string_literal_text(value, dialect) {
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
    if let Some(text) = concatenated_literal_text(value, dialect) {
        return Ok(Some(text));
    }
    if let Some(text) = unwrap_conversion_call(value, dialect)? {
        return Ok(Some(text));
    }
    Err(format!(
        "The SQL file has a value this import cannot read: {value}"
    ))
}

/// Decode `'a'||CHR(38)||'b'` — the shape the Oracle literal writer produces so
/// an `&` in the data cannot be read as a substitution variable.
///
/// The writer's exact inverse. Without it, the `SQL Inserts` export of any value
/// holding an `&` would no longer read back into the importer that produced it.
/// Only for an Oracle-dialect file: the MySQL family has no substitution to
/// defuse, so the writer never emits this shape for it, and `||` means something
/// else there.
fn concatenated_literal_text(value: &str, dialect: SqlFileDialect) -> Option<String> {
    if dialect != SqlFileDialect::Oracle {
        return None;
    }
    // No minimum part count: a value that is nothing but `&` is written as the
    // single expression `CHR(38)`. A one-part expression that is a plain literal
    // never reaches here — `sql_value_text` reads that first — so the only
    // one-part shape left is a `CHR` call.
    let mut out = String::new();
    for part in split_concatenation(value, dialect) {
        let part = part.trim();
        // The other shape the writer joins with `||`: a value longer than one
        // Oracle literal is written as `TO_CLOB('…')||TO_CLOB('…')`, because
        // `ORA-01704` is what a single literal past 4000 bytes gets. Reading it
        // back is what keeps that export re-importable. Its argument is a
        // concatenation of its own when the piece held an `&`, and nothing
        // deeper than that, so this does not recurse.
        let text = match to_clob_argument(part) {
            Some(inner) => simple_concatenation_text(inner, dialect)?,
            None => concatenation_piece_text(part, dialect)?,
        };
        out.push_str(&text);
    }
    Some(out)
}

/// The text inside a `TO_CLOB(…)` call, or `None` when this is not one.
///
/// The writer's wrapper for one piece of a value too long for a single Oracle
/// literal. Only the outermost call is stripped: what it holds is a plain
/// literal, or the `'a'||CHR(38)||'b'` a defused `&` makes of one, and never
/// another `TO_CLOB` — so reading it needs no recursion and cannot be made to.
///
/// Matched the way every other call name here is, without regard to case.
fn to_clob_argument(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    if !text[..open].trim().eq_ignore_ascii_case("TO_CLOB") {
        return None;
    }
    text.strip_suffix(')').map(|rest| &rest[open + 1..])
}

/// One piece of a concatenation: a plain literal, or a `CHR(n)` call.
///
/// The ONE piece-reader, so the top-level chain and whatever a `TO_CLOB` wraps
/// cannot come to disagree about what a piece is.
fn concatenation_piece_text(part: &str, dialect: SqlFileDialect) -> Option<String> {
    if let Some(text) = sql_string_literal_text(part, dialect) {
        return Some(text);
    }
    chr_call_character(part).map(String::from)
}

/// The text of a `||` chain of plain literals and `CHR(n)` calls.
fn simple_concatenation_text(value: &str, dialect: SqlFileDialect) -> Option<String> {
    split_concatenation(value, dialect)
        .into_iter()
        .map(|part| concatenation_piece_text(part.trim(), dialect))
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.concat())
}

/// The character a `CHR(<code>)` call produces, for the concatenation reader.
fn chr_call_character(text: &str) -> Option<char> {
    let head = text.get(..4)?;
    if !head.eq_ignore_ascii_case("CHR(") {
        return None;
    }
    let code: u32 = text[4..].strip_suffix(')')?.trim().parse().ok()?;
    char::from_u32(code)
}

/// Decode `'…'`, undoing the doubled quote every dialect uses and, in a
/// MySQL-family file, the backslash escapes too.
///
/// `None` unless the literal is the WHOLE expression, asked of the same scanner
/// everything else uses. Stripping the first and last quote is not enough:
/// `'a'||CHR(38)||'b'` also begins and ends with one, and reading it as a single
/// literal produced text that was neither the value nor an error.
fn sql_string_literal_text(value: &str, dialect: SqlFileDialect) -> Option<String> {
    if value.len() < 2 || !value.starts_with('\'') || !value.ends_with('\'') {
        return None;
    }
    if string_literal_end(value, 0, dialect) != value.len() {
        return None;
    }
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
            // MySQL's own escape table, whole. It used to hold four of the
            // ten rows and drop the backslash from everything else, which
            // turned `\Z` into the letter `Z` and `\%` into a bare `%` — a
            // silent rewrite of any file this app did not write itself, and
            // `SQL Inserts` import advertises reading a mysqldump.
            '\\' if dialect.backslash_escapes() => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('0') => out.push('\0'),
                Some('b') => out.push('\u{8}'),
                // `\Z` is Ctrl-Z, which Windows reads as end-of-file; MySQL
                // escapes it for that reason and stores the character itself.
                Some('Z') => out.push('\u{1A}'),
                // The two the server does NOT unescape: inside `LIKE` they are
                // the literal wildcards, so the backslash is part of the value.
                Some(next @ ('%' | '_')) => {
                    out.push('\\');
                    out.push(next);
                }
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
fn unwrap_conversion_call(value: &str, dialect: SqlFileDialect) -> Result<Option<String>, String> {
    let Some(open) = value.find('(') else {
        return Ok(None);
    };
    let name = value[..open].trim();
    let known = [
        "to_date",
        "to_timestamp",
        "to_timestamp_tz",
        "hextoraw",
        "to_clob",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate));
    if !known {
        return Ok(None);
    }
    let close = matching_paren(value, open, dialect)
        .ok_or_else(|| format!("A conversion call is not closed: {value}"))?;
    if value[close + 1..].trim().is_empty() {
        // The first argument is the value; a format model, if present, is the
        // one the exporter chose and carries no data.
        if let Some(first) = split_top_level(&value[open + 1..close], dialect).first() {
            if let Some(text) = sql_string_literal_text(first.trim(), dialect) {
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
    use crate::ui::result_export::{render, ExportCell, ExportContent, ExportGrid};

    /// The SQL a builder wrote, for a fixture it must not refuse.
    ///
    /// Every selection here is one this app can write; a refusal would be a
    /// defect in the writer, and its sentence is what to fail with.
    fn written(built: ExportContent) -> String {
        match built.into_parts() {
            Ok((text, _)) => text,
            Err(reason) => panic!("the SQL Inserts builder refused a fixture: {reason}"),
        }
    }

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
            // The second row's CODE is SQL NULL. The fixture used to spell
            // that as the TEXT `NULL`, which is how the grid DISPLAYS one; the
            // snapshot the grid hands the serializers states it as an absent
            // value, and so does this.
            rows: vec![
                vec![
                    Some("1".to_string()),
                    Some("a,b\t\"c\"\nd\re|f\\g<h>&i".to_string()),
                    Some("00123".to_string()),
                ],
                vec![Some("2".to_string()), Some("한글".to_string()), None],
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
            dialect: crate::ui::grid_sql_export::SqlWriteDialect::family_default(db_type),
            table: Some("T".to_string()),
            all_columns: grid.columns.clone(),
            column_kinds: grid.column_kinds.clone(),
            selected_columns: (0..grid.columns.len()).collect(),
            rows: grid.rows.clone(),
        }
    }

    #[test]
    fn sql_inserts_round_trip_on_oracle() {
        let sql = written(build_sql_inserts(&selection(DatabaseType::Oracle)));
        assert_eq!(
            parse(&sql, &options(ExportFormat::SqlInserts)).expect("parses"),
            expected("a,b\t\"c\"\nd\re|f\\g<h>&i")
        );
    }

    #[test]
    fn sql_inserts_round_trip_on_mysql() {
        // MySQL literals carry backslash escapes and backtick-quoted columns.
        let sql = written(build_sql_inserts(&selection(DatabaseType::MySQL)));
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

    /// A BLANK LINE is not a row.
    ///
    /// It used to be a record of one empty field, which `validate` then padded
    /// out to the file's full width — so a file with a trailing empty line, or
    /// one left behind by an editor, imported an extra row of NULLs. On a table
    /// with a NOT NULL column that row failed and took the whole import with
    /// it; on one without, it landed silently.
    #[test]
    fn a_blank_line_is_not_a_row() {
        for ending in ["\n", "\r\n", "\r"] {
            let text = format!("A,B{ending}1,2{ending}{ending}");
            let table = parse(&text, &options(ExportFormat::Csv))
                .unwrap_or_else(|error| panic!("{ending:?}: {error}"));
            assert_eq!(
                table.rows,
                vec![vec![Some("1".to_string()), Some("2".to_string())]],
                "a trailing blank line ({ending:?}) became a row"
            );

            // And one in the MIDDLE is not a row either.
            let text = format!("A,B{ending}{ending}1,2{ending}");
            let table = parse(&text, &options(ExportFormat::Csv))
                .unwrap_or_else(|error| panic!("{ending:?}: {error}"));
            assert_eq!(
                table.rows.len(),
                1,
                "a blank line ({ending:?}) became a row"
            );
        }

        // A line that really holds one empty VALUE says so by quoting it, and
        // that is still a row.
        let table = parse("A\n\"\"\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some(String::new())]]);

        // So is a line of empty fields: the separator says how many there are.
        let table = parse("A,B\n,\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some(String::new()), Some(String::new())]]
        );
    }

    /// The NULL text QUOTED is a value; bare, it is SQL NULL.
    ///
    /// The writer's exact inverse ([`crate::ui::result_export::ExportGrid`]
    /// quotes a value that spells the NULL text). Before this, both spellings
    /// read as NULL, so a `VARCHAR` holding the four letters `NULL` could not
    /// survive this app's own export → import at all.
    #[test]
    fn a_quoted_null_text_is_a_value_and_a_bare_one_is_null() {
        let table = parse("A\nNULL\n\"NULL\"\n", &options(ExportFormat::Csv)).expect("parses");
        assert_eq!(table.rows, vec![vec![None], vec![Some("NULL".to_string())]]);

        // An empty NULL text works the same way: bare is NULL, `""` is the
        // empty string — which is the only way a delimited file can say it.
        //
        // Two columns, because a ONE-column row of nothing but an empty
        // unquoted field is a blank line byte for byte, and no reader can tell
        // those apart — see `split_delimited_records`.
        let empty_means_null = ImportOptions {
            format: ExportFormat::Csv,
            has_header: true,
            null_text: String::new(),
        };
        let table = parse("A,B\n,\"\"\n", &empty_means_null).expect("parses");
        assert_eq!(table.rows, vec![vec![None, Some(String::new())]]);
    }

    /// A NULL text that needs quotes of its own spends the signal, and the
    /// reader falls back to matching the text — which is what it always did,
    /// so no file loses a NULL it used to keep.
    #[test]
    fn a_null_text_that_must_be_quoted_still_reads_as_null() {
        let comma_null = ImportOptions {
            format: ExportFormat::Csv,
            has_header: true,
            null_text: "a,b".to_string(),
        };
        let table = parse("A\n\"a,b\"\n", &comma_null).expect("parses");
        assert_eq!(table.rows, vec![vec![None]]);
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
        // `.text` names plain text, like `.txt`; it used to preselect Markdown
        // while `.txt` preselected CSV, which was a difference with no reason.
        assert_eq!(detect_format(Path::new("a.text")), Some(ExportFormat::Csv));
        assert_eq!(detect_format(Path::new("a.txt")), Some(ExportFormat::Csv));
        assert_eq!(
            detect_format(Path::new("a.markdown")),
            Some(ExportFormat::Markdown)
        );
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
                Some("1".to_string()),
                Some("comma, and \"quotes\"".to_string()),
                Some("00123".to_string()),
                Some("1".to_string()),
                Some("-12.5".to_string()),
                Some("line1\nline2".to_string()),
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
        // CSV addresses a column by POSITION, so a repeated name is not a
        // collision and nothing has to be renamed.
        assert_eq!(table.columns, hostile_columns());
        assert_eq!(table.rows[0][1], Some("comma, and \"quotes\"".to_string()));
        assert_eq!(table.rows[0][5], Some("line1\nline2".to_string()));
    }

    /// A repeated column name is made unique by the writer, so no column is
    /// lost.
    ///
    /// This used to assert the opposite — that the second `ID` collapsed into
    /// the first, leaving five columns for six — because a JSON object really
    /// does have one value per name and the writer emitted the name twice.
    /// Duplicate result column names are ordinary (`SELECT a.id, b.id …`), and
    /// silently dropping a column of the user's data is not an acceptable
    /// reading of "export"; the writer now suffixes the repeat instead.
    #[test]
    fn json_carries_hostile_column_names_through_unchanged() {
        let table = parse(
            &render(ExportFormat::Json, &hostile_name_grid()),
            &options(ExportFormat::Json),
        )
        .expect("parses");
        assert_eq!(
            table.columns,
            vec![
                "ID".to_string(),
                "FULL NAME".to_string(),
                "COUNT(*)".to_string(),
                "ID_2".to_string(),
                "2024_TOTAL".to_string(),
                "NOTE".to_string(),
            ]
        );
        // Every value is still there, in its own column.
        assert_eq!(
            table.rows[0],
            vec![
                Some("1".to_string()),
                Some("comma, and \"quotes\"".to_string()),
                Some("00123".to_string()),
                Some("1".to_string()),
                Some("-12.5".to_string()),
                Some("line1\nline2".to_string()),
            ]
        );
    }

    /// XML cannot carry `COUNT(*)` or a name starting with a digit, so the
    /// export rewrote them, and that is what comes back.
    ///
    /// Sanitizing can also CREATE a repeat out of two different names, so the
    /// same uniqueness rule runs after it — this used to lose the duplicate
    /// `ID` for the same reason JSON did.
    #[test]
    fn xml_reports_the_element_names_the_export_had_to_invent() {
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
                "ID_2".to_string(),
                "column_5".to_string(),
                "NOTE".to_string(),
            ]
        );
        assert_eq!(table.rows[0][2], Some("00123".to_string()));
        assert_eq!(table.rows[0][5], Some("line1\nline2".to_string()));
    }

    /// An Oracle-dialect file keeps every row when a value ends in a
    /// backslash.
    ///
    /// Oracle reads `\` as an ordinary character, so `'C:\path\'` is a
    /// complete literal. The reader used to apply the MySQL escape rule while
    /// SPLITTING (it only chose the dialect afterwards, per statement), so the
    /// closing quote was swallowed as an escaped one and the rest of the file
    /// became part of that value: two statements parsed as one row, with no
    /// error. `'a\''b'` was worse — it parsed, and gave `a\`.
    #[test]
    fn an_oracle_backslash_does_not_swallow_the_statements_after_it() {
        for (value, sql) in [
            (
                "C:\\path\\",
                "INSERT INTO T (ID, N) VALUES (1, 'C:\\path\\');\n\
                 INSERT INTO T (ID, N) VALUES (2, 'second');\n",
            ),
            (
                "a\\'b",
                "INSERT INTO T (ID, N) VALUES (1, 'a\\''b');\n\
                 INSERT INTO T (ID, N) VALUES (2, 'second');\n",
            ),
            (
                "trailing\\",
                "INSERT INTO T (ID, N) VALUES (1, 'trailing\\');\n\
                 INSERT INTO T (ID, N) VALUES (2, 'second');\n",
            ),
        ] {
            let table = parse(sql, &options(ExportFormat::SqlInserts))
                .unwrap_or_else(|error| panic!("{value:?}: {error}"));
            assert_eq!(
                table.rows,
                vec![
                    vec![Some("1".to_string()), Some(value.to_string())],
                    vec![Some("2".to_string()), Some("second".to_string())],
                ],
                "{value:?}"
            );
        }
    }

    /// The same values in the MySQL family's spelling, which is a different
    /// spelling of the same data — the backticks are what say so.
    #[test]
    fn a_mysql_file_still_reads_its_own_backslash_escapes() {
        for (value, sql) in [
            (
                "C:\\path\\",
                "INSERT INTO `T` (`ID`, `N`) VALUES (1, 'C:\\\\path\\\\');\n\
                 INSERT INTO `T` (`ID`, `N`) VALUES (2, 'second');\n",
            ),
            (
                "a\\'b",
                "INSERT INTO `T` (`ID`, `N`) VALUES (1, 'a\\\\''b');\n\
                 INSERT INTO `T` (`ID`, `N`) VALUES (2, 'second');\n",
            ),
        ] {
            let table = parse(sql, &options(ExportFormat::SqlInserts))
                .unwrap_or_else(|error| panic!("{value:?}: {error}"));
            assert_eq!(
                table.rows,
                vec![
                    vec![Some("1".to_string()), Some(value.to_string())],
                    vec![Some("2".to_string()), Some("second".to_string())],
                ],
                "{value:?}"
            );
        }
    }

    /// The entity bound gates the SEARCH, not the result.
    ///
    /// The values are unchanged by the fix — this pins the shapes the reader
    /// must still resolve, and the shapes it must still leave alone — while
    /// `an_ampersand_run_without_a_terminator_stays_linear` pins the cost.
    #[test]
    fn an_entity_is_read_only_within_reach_of_its_ampersand() {
        // Real entities, the longest numeric form included.
        assert_eq!(decode_entities("a&amp;b"), "a&b");
        assert_eq!(decode_entities("&lt;&gt;&quot;&apos;&nbsp;"), "<>\"'\u{A0}");
        assert_eq!(decode_entities("&#65;&#x41;&#x10FFFF;"), "AA\u{10FFFF}");
        // A `;` further away than any entity body is not a terminator, and the
        // `&` is data.
        assert_eq!(
            decode_entities("&not_an_entity_at_all;"),
            "&not_an_entity_at_all;"
        );
        // A bare `&` with no `;` anywhere is data.
        assert_eq!(decode_entities("a & b && c"), "a & b && c");
        assert_eq!(decode_entities("&&&"), "&&&");
        // Text with no `&` at all is returned untouched.
        assert_eq!(decode_entities("plain"), "plain");
    }

    /// A file this app did not write can be dense in bare `&`, and reading it
    /// must not be quadratic.
    ///
    /// The bound was applied AFTER `find(';')` had scanned the whole remaining
    /// text, so every `&` in a run scanned to the end of the file: 40 KB took
    /// 50 ms, 80 KB 131 ms, 160 KB 517 ms, 320 KB 2069 ms — ×4 per doubling, on
    /// the UI thread inside the import dialog.
    ///
    /// Timed rather than counted because the cost is the defect. The ratio is
    /// what is asserted, not a duration: doubling the input may not halve the
    /// per-byte cost on a loaded machine, but it may not QUADRUPLE the total
    /// either. The threshold is deliberately loose — quadratic growth is ×4 and
    /// this refuses at ×2.5 — so the test measures the shape and not the box.
    #[test]
    fn an_ampersand_run_without_a_terminator_stays_linear() {
        let time = |n: usize| {
            let text = "&".repeat(n);
            let start = std::time::Instant::now();
            let decoded = decode_entities(&text);
            assert_eq!(decoded.len(), n, "every bare ampersand is data");
            start.elapsed().as_secs_f64()
        };
        // Warm the allocator so the first measurement is not the odd one.
        let _ = time(200_000);
        let small = time(200_000);
        let large = time(400_000);
        assert!(
            large < small * 2.5 + 0.002,
            "twice the input took {large:.4}s against {small:.4}s for half — \
             that is the quadratic scan, not linear work"
        );
    }

    /// The reader is the writer's inverse for a value longer than one literal.
    ///
    /// A value past Oracle's 4000-byte literal limit is exported as
    /// `TO_CLOB('…')||TO_CLOB('…')`; if the reader did not know that shape, the
    /// app could write a `SQL Inserts` file it could not read back.
    #[test]
    fn a_long_value_round_trips_through_the_concatenated_form() {
        use crate::ui::grid_sql_export::{sql_literal_for_value, SqlWriteDialect};

        let dialect = SqlWriteDialect::family_default(DatabaseType::Oracle);
        for value in [
            "y".repeat(10_000),
            format!("{}&T", "z".repeat(5_000)),
            format!("it{}s", "'".repeat(4_100)),
            "한국어".repeat(2_000),
        ] {
            let literal = sql_literal_for_value(dialect, SqlValueKind::String, &value)
                .expect("text is written as a concatenation, however long it is");
            assert!(
                literal.contains("TO_CLOB("),
                "not chunked: {}",
                &literal[..30]
            );
            let sql = format!("INSERT INTO T (V) VALUES ({literal});\n");
            let table = parse(&sql, &options(ExportFormat::SqlInserts))
                .unwrap_or_else(|error| panic!("{}: {error}", &value[..20]));
            assert_eq!(
                table.rows,
                vec![vec![Some(value.clone())]],
                "a value of {} chars did not come back",
                value.chars().count()
            );
        }
    }

    /// The whole of MySQL's escape table, not the four rows it used to hold.
    ///
    /// `SQL Inserts` import advertises reading a MySQL-family file, and a file
    /// this app did not write — a `mysqldump` — carries the other six. The
    /// server's own values are what these are measured against:
    /// `HEX('a\Zb')` is `611A62`, `HEX('a\bb')` is `610862`, and
    /// `HEX('a\%b')` is `615C2562` — the backslash SURVIVES before `%` and
    /// `_`, because inside `LIKE` those are the wildcards.
    #[test]
    fn a_mysql_file_reads_every_escape_the_server_defines() {
        for (escaped, expected) in [
            ("a\\0b", "a\0b"),
            ("a\\bb", "a\u{8}b"),
            ("a\\nb", "a\nb"),
            ("a\\rb", "a\rb"),
            ("a\\tb", "a\tb"),
            ("a\\Zb", "a\u{1A}b"),
            ("a\\\\b", "a\\b"),
            ("a\\%b", "a\\%b"),
            ("a\\_b", "a\\_b"),
            // Anything else is the character itself, which is also the
            // server's rule.
            ("a\\qb", "aqb"),
        ] {
            let sql = format!("INSERT INTO `T` (`ID`, `N`) VALUES (1, '{escaped}');\n");
            let table = parse(&sql, &options(ExportFormat::SqlInserts))
                .unwrap_or_else(|error| panic!("{escaped:?}: {error}"));
            assert_eq!(
                table.rows,
                vec![vec![Some("1".to_string()), Some(expected.to_string())]],
                "{escaped:?}"
            );
        }
    }

    /// An Oracle file reads a backslash as itself, every one of those rows
    /// included: only the MySQL family has the escape table at all.
    #[test]
    fn an_oracle_file_reads_no_backslash_escape() {
        let sql = "INSERT INTO T (ID, N) VALUES (1, 'a\\Zb\\nc');\n";
        let table = parse(sql, &options(ExportFormat::SqlInserts)).expect("parses");
        assert_eq!(
            table.rows,
            vec![vec![Some("1".to_string()), Some("a\\Zb\\nc".to_string())]]
        );
    }

    /// A `SQL Inserts` export of an Oracle grid is safe to RUN as well as to
    /// re-import.
    ///
    /// This app substitutes `&name` inside string literals the way SQL*Plus
    /// does, and `DEFINE` is on by default, so a plain `'R&D'` in the exported
    /// file stops the run and asks the user to enter a value. The writer lifts
    /// the `&` out; this pins that the reader still gets the value back.
    #[test]
    fn an_ampersand_survives_the_oracle_sql_export_and_import_round_trip() {
        let grid = ExportGrid {
            columns: vec!["N".to_string()],
            column_kinds: vec![SqlValueKind::String],
            rows: vec![
                vec![Some("R&D".to_string())],
                vec![Some("&start".to_string())],
                vec![Some("end&".to_string())],
                vec![Some("a&&b".to_string())],
                vec![Some("&".to_string())],
            ],
            null_text: NULL_TEXT.to_string(),
        };
        let selection = GridSqlSelection {
            dialect: crate::ui::grid_sql_export::SqlWriteDialect::family_default(
                DatabaseType::Oracle,
            ),
            table: Some("T".to_string()),
            all_columns: grid.columns.clone(),
            column_kinds: grid.column_kinds.clone(),
            selected_columns: vec![0],
            rows: grid.rows.clone(),
        };
        let sql = written(build_sql_inserts(&selection));
        assert!(
            !sql.contains('&'),
            "an exported Oracle literal may not carry a bare &: {sql}"
        );
        assert_eq!(
            parse(&sql, &options(ExportFormat::SqlInserts))
                .expect("parses")
                .rows,
            grid.rows
        );
    }

    /// The four letters `NULL` survive this app's own CSV round trip.
    ///
    /// Writer and reader held together: the export quotes a value that spells
    /// the NULL text, and the import reads the quoting. Before, both were the
    /// same bytes and the value came back as SQL NULL — on every backend, from
    /// the app's own file.
    #[test]
    fn a_value_spelling_the_null_text_survives_the_csv_round_trip() {
        for format in [ExportFormat::Csv, ExportFormat::Tsv] {
            let grid = ExportGrid {
                columns: vec!["V".to_string(), "W".to_string()],
                column_kinds: vec![SqlValueKind::String, SqlValueKind::String],
                rows: vec![
                    vec![None, Some("keep".to_string())],
                    vec![Some(NULL_TEXT.to_string()), Some(String::new())],
                ],
                null_text: NULL_TEXT.to_string(),
            };
            let text = render(format, &grid);
            let table = parse(&text, &options(format))
                .unwrap_or_else(|error| panic!("{}: {error}", format.label()));
            assert_eq!(
                table.rows,
                vec![
                    vec![None, Some("keep".to_string())],
                    vec![Some(NULL_TEXT.to_string()), Some(String::new())],
                ],
                "{} did not round trip",
                format.label()
            );
        }
    }

    /// A row of ONE column holding the empty string survives the round trip.
    ///
    /// The blank-line rule and the empty string meet here: written bare, such a
    /// row IS a blank line, and dropping blank lines would drop it. The writer
    /// quotes an empty value for exactly that reason, and this holds the two
    /// halves together.
    #[test]
    fn a_single_column_row_of_the_empty_string_is_not_a_blank_line() {
        for format in [ExportFormat::Csv, ExportFormat::Tsv] {
            let grid = ExportGrid {
                columns: vec!["V".to_string()],
                column_kinds: vec![SqlValueKind::String],
                rows: vec![
                    vec![Some(String::new())],
                    vec![Some("x".to_string())],
                    vec![None],
                ],
                null_text: NULL_TEXT.to_string(),
            };
            let table = parse(&render(format, &grid), &options(format))
                .unwrap_or_else(|error| panic!("{}: {error}", format.label()));
            assert_eq!(
                table.rows,
                vec![
                    vec![Some(String::new())],
                    vec![Some("x".to_string())],
                    vec![None],
                ],
                "{} lost the empty-string row",
                format.label()
            );
        }
    }

    /// One statement carries MANY rows, and every one of them has to arrive.
    ///
    /// The reader used to take the first `(…)` after `VALUES` and drop the rest
    /// with no error — so a `mysqldump` file, and the very script this app's own
    /// import builder writes, came back holding one row per statement.
    #[test]
    fn every_row_of_a_multi_row_statement_is_read() {
        for (label, sql, expected) in [
            (
                "a multi-row VALUES list",
                "INSERT INTO `t` (`id`,`v`) VALUES (1,'a'),(2,'b'),(3,'c');",
                3,
            ),
            (
                "an Oracle INSERT ALL",
                "INSERT ALL INTO t (id, v) VALUES (1,'a') INTO t (id, v) VALUES (2,'b') \
                 INTO t (id, v) VALUES (3,'c') SELECT * FROM DUAL;",
                3,
            ),
            (
                // A trailing clause changes what the statement DOES, not which
                // rows it carries, so the rows are still read.
                "a trailing ON DUPLICATE KEY UPDATE",
                "INSERT INTO `t` (`id`,`v`) VALUES (1,'a'),(2,'b') \
                 ON DUPLICATE KEY UPDATE v = 'x';",
                2,
            ),
            (
                "a trailing RETURNING",
                "INSERT INTO t (id, v) VALUES (1,'a'),(2,'b') RETURNING id;",
                2,
            ),
        ] {
            let table = parse(sql, &options(ExportFormat::SqlInserts))
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(table.rows.len(), expected, "{label}");
        }
    }

    /// The script this app writes for an import reads back as the rows it was
    /// built from — at any batch size, on every dialect.
    #[test]
    fn the_import_script_this_app_writes_reads_back_whole() {
        use crate::db::DatabaseType;
        use crate::ui::grid_sql_export::SqlWriteDialect;
        use crate::ui::table_import::{
            build_insert_script, default_mapping, ImportRequest, TargetColumn,
        };

        let targets = vec![
            TargetColumn {
                name: "ID".to_string(),
                kind: SqlValueKind::Number,
                nullable: true,
            },
            TargetColumn {
                name: "V".to_string(),
                kind: SqlValueKind::String,
                nullable: true,
            },
        ];
        let data = ImportedTable {
            columns: vec!["ID".to_string(), "V".to_string()],
            rows: (1..=5)
                .map(|index| vec![Some(index.to_string()), Some(format!("row {index}"))])
                .collect(),
        };
        let mapping = default_mapping(&data.columns, &targets);
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            for batch_rows in [100, 2, 1] {
                let script = build_insert_script(&ImportRequest {
                    dialect: SqlWriteDialect::family_default(db_type),
                    table: "T",
                    targets: &targets,
                    mapping: &mapping,
                    data: &data,
                    batch_rows,
                })
                .expect("builds");
                let back = parse(&script, &options(ExportFormat::SqlInserts))
                    .unwrap_or_else(|error| panic!("{db_type} batch {batch_rows}: {error}"));
                assert_eq!(back.rows, data.rows, "{db_type} batch {batch_rows}");
            }
        }
    }

    /// A shape whose row count this reader cannot know is refused BY NAME, not
    /// read as if it carried one row.
    #[test]
    fn a_statement_this_reader_cannot_count_is_refused() {
        for (label, sql, needle) in [
            (
                "INSERT FIRST routes rows by its WHEN clauses",
                "INSERT FIRST WHEN a > 1 THEN INTO t (a) VALUES (1) SELECT * FROM DUAL;",
                "INSERT FIRST",
            ),
            (
                "an INSERT ALL driven by a real query",
                "INSERT ALL INTO t (a) VALUES (1) SELECT id FROM src;",
                "cannot count rows for",
            ),
            (
                "a comma that leads nowhere",
                "INSERT INTO t (a) VALUES (1), x;",
                "not followed by another row",
            ),
            (
                "targets that name different columns",
                "INSERT ALL INTO t (a) VALUES (1) INTO t (b) VALUES (2) SELECT * FROM DUAL;",
                "same column list",
            ),
        ] {
            let error = parse(sql, &options(ExportFormat::SqlInserts))
                .expect_err(&format!("{label} must be refused"));
            assert!(error.contains(needle), "{label}: {error}");
        }
    }

    /// The same columns in another ORDER name the same row.
    ///
    /// Such a file used to be refused outright. It is read now, and each
    /// statement's values are lifted into the first statement's column order —
    /// the assertion that matters, because getting the ORDER wrong would put
    /// every value in the wrong column instead of refusing.
    #[test]
    fn statements_may_name_the_same_columns_in_another_order() {
        let table = parse(
            "INSERT INTO T (A,B) VALUES (1,2);\n\
             INSERT INTO T (B,A) VALUES (30,40);\n\
             INSERT ALL INTO T (B,A) VALUES (50,60) SELECT * FROM DUAL;",
            &options(ExportFormat::SqlInserts),
        )
        .expect("parses");
        assert_eq!(table.columns, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            table.rows,
            vec![
                vec![Some("1".to_string()), Some("2".to_string())],
                vec![Some("40".to_string()), Some("30".to_string())],
                vec![Some("60".to_string()), Some("50".to_string())],
            ]
        );
        // A different SET of columns is still a file this cannot read as one
        // table.
        assert!(parse(
            "INSERT INTO T (A,B) VALUES (1,2);\nINSERT INTO T (A,C) VALUES (3,4);",
            &options(ExportFormat::SqlInserts)
        )
        .is_err());
    }

    /// A keyword spelled inside a value or a name is not a keyword.
    #[test]
    fn a_literal_that_spells_a_keyword_is_still_a_value() {
        let table = parse(
            "INSERT ALL INTO t (a) VALUES ('x INTO y') INTO t (a) VALUES ('has VALUES (9)') \
             SELECT * FROM DUAL;",
            &options(ExportFormat::SqlInserts),
        )
        .expect("parses");
        assert_eq!(
            table.rows,
            vec![
                vec![Some("x INTO y".to_string())],
                vec![Some("has VALUES (9)".to_string())],
            ]
        );
    }

    /// A cell that spans columns fills every one of them, and a cell that spans
    /// rows re-appears in the rows below.
    ///
    /// Reading one cell as one column shifted every later cell of a spanned row
    /// one place to the left, silently, into the wrong column of the table being
    /// loaded.
    #[test]
    fn html_spans_keep_every_value_in_its_own_column() {
        for (label, html, expected) in [
            (
                "colspan fills the columns it covers",
                "<table><tr><th>A</th><th>B</th><th>C</th></tr>\
                 <tr><td colspan='2'>x</td><td>y</td></tr></table>",
                vec![vec![
                    Some("x".to_string()),
                    Some("x".to_string()),
                    Some("y".to_string()),
                ]],
            ),
            (
                "rowspan re-appears in the row below",
                "<table><tr><th>A</th><th>B</th></tr>\
                 <tr><td rowspan=\"2\">x</td><td>y</td></tr><tr><td>z</td></tr></table>",
                vec![
                    vec![Some("x".to_string()), Some("y".to_string())],
                    vec![Some("x".to_string()), Some("z".to_string())],
                ],
            ),
            (
                // A `<tr>` with no cells of its own is still a row when a
                // rowspan owes it a value; skipping it handed that value to the
                // NEXT row, one row too early.
                "an empty row still consumes a row of the span",
                "<table><tr><th>A</th><th>B</th></tr>\
                 <tr><td rowspan=3>x</td><td>1</td></tr><tr></tr><tr><td>3</td></tr></table>",
                vec![
                    vec![Some("x".to_string()), Some("1".to_string())],
                    vec![Some("x".to_string()), None],
                    vec![Some("x".to_string()), Some("3".to_string())],
                ],
            ),
            (
                "a rowspan in the LAST column still lands there",
                "<table><tr><th>A</th><th>B</th></tr>\
                 <tr><td>y</td><td rowspan=2>x</td></tr><tr><td>z</td></tr></table>",
                vec![
                    vec![Some("y".to_string()), Some("x".to_string())],
                    vec![Some("z".to_string()), Some("x".to_string())],
                ],
            ),
        ] {
            let table = parse(html, &options(ExportFormat::Html))
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(table.rows, expected, "{label}");
        }
    }

    /// A `<table>` opens a scope the implicit-close rules do not cross.
    ///
    /// `<tr>` closes a previous `<tr>` — but only a SIBLING one. Without the
    /// scope, the row of a nested table closed the outer row and was promoted
    /// beside it, leaving the outer cell empty.
    #[test]
    fn a_nested_html_table_is_the_text_of_its_cell() {
        let table = parse(
            "<table><tr><th>A</th></tr>\
             <tr><td><table><tr><td>inner</td></tr></table></td></tr></table>",
            &options(ExportFormat::Html),
        )
        .expect("parses");
        assert_eq!(table.columns, vec!["A".to_string()]);
        assert_eq!(table.rows, vec![vec![Some("inner".to_string())]]);
    }

    /// A span a file asks for is clamped to what HTML itself allows, so a
    /// number in a file cannot ask this reader for an arbitrary allocation.
    #[test]
    fn an_absurd_span_is_clamped_and_then_reported() {
        let error = parse(
            "<table><tr><th>A</th></tr><tr><td colspan=99999999>x</td></tr></table>",
            &options(ExportFormat::Html),
        )
        .expect_err("refused");
        assert!(error.contains("1000 values"), "{error}");
        // A span of zero or none is one column, and an attribute whose name
        // merely ENDS in `colspan` is not one.
        for html in [
            "<table><tr><th>A</th><th>B</th></tr><tr><td colspan=0>1</td><td>2</td></tr></table>",
            "<table><tr><th>A</th><th>B</th></tr>\
             <tr><td data-colspan=2>1</td><td>2</td></tr></table>",
        ] {
            let table = parse(html, &options(ExportFormat::Html)).expect("parses");
            assert_eq!(
                table.rows,
                vec![vec![Some("1".to_string()), Some("2".to_string())]],
                "{html}"
            );
        }
    }

    /// The LAST entry wins when one JSON object repeats a key, which is what
    /// `serde_json` — the parser the rest of this app uses — does with the same
    /// document.
    #[test]
    fn a_repeated_json_key_reads_as_its_last_value() {
        let table = parse("[{\"A\":1,\"A\":2}]", &options(ExportFormat::Json)).expect("parses");
        assert_eq!(table.rows, vec![vec![Some("2".to_string())]]);
    }

    /// A `<br>` that was in the DATA is not a line break.
    ///
    /// The writer escapes every `<`, so only the marker it added itself has a
    /// bare one. The reader used to substitute `<br>` in a pass of its own,
    /// after unescaping, by which point the two were spelled the same.
    #[test]
    fn markdown_keeps_a_literal_br_out_of_the_line_breaks() {
        let grid = ExportGrid {
            columns: vec!["V".to_string()],
            column_kinds: vec![SqlValueKind::String],
            rows: vec![
                vec![Some("a<br>b".to_string())],
                vec![Some("a\nb".to_string())],
                vec![Some("<tag> & \\pipe\\ |".to_string())],
            ],
            null_text: NULL_TEXT.to_string(),
        };
        assert_eq!(
            parse(
                &render(ExportFormat::Markdown, &grid),
                &options(ExportFormat::Markdown)
            )
            .expect("parses")
            .rows,
            grid.rows
        );
    }

    /// Two columns whose names only COLLIDE after XML sanitizing still come
    /// back as two columns.
    #[test]
    fn xml_separates_names_that_only_collide_once_sanitized() {
        let grid = ExportGrid {
            columns: vec!["A(B".to_string(), "A)B".to_string()],
            column_kinds: vec![SqlValueKind::String, SqlValueKind::String],
            rows: vec![vec![Some("one".to_string()), Some("two".to_string())]],
            null_text: NULL_TEXT.to_string(),
        };
        let table = parse(
            &render(ExportFormat::Xml, &grid),
            &options(ExportFormat::Xml),
        )
        .expect("parses");
        assert_eq!(table.columns, vec!["A_B".to_string(), "A_B_2".to_string()]);
        assert_eq!(
            table.rows,
            vec![vec![Some("one".to_string()), Some("two".to_string())]]
        );
    }

    #[test]
    fn a_number_keeps_its_exact_spelling_through_json() {
        // `serde_json::Value` would turn this into `12000000000.0`.
        let grid = ExportGrid {
            columns: vec!["N".to_string(), "M".to_string()],
            column_kinds: vec![SqlValueKind::Number, SqlValueKind::Number],
            rows: vec![vec![
                Some("1.2E+10".to_string()),
                Some("-0.000001".to_string()),
            ]],
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
        let mysql = written(build_sql_inserts(&selection(DatabaseType::MySQL)));
        let oracle = written(build_sql_inserts(&selection(DatabaseType::Oracle)));
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
        let rows: Vec<Vec<ExportCell>> = (0..1000)
            .map(|index| {
                vec![
                    Some(index.to_string()),
                    Some(format!("name |{index}, \"q\"\n{index}")),
                    // Every seventh CODE is SQL NULL.
                    (index % 7 != 0).then(|| format!("{index:08}")),
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
                written(build_sql_inserts(&GridSqlSelection {
                    dialect: crate::ui::grid_sql_export::SqlWriteDialect::family_default(
                        DatabaseType::Oracle,
                    ),
                    table: Some("T".to_string()),
                    all_columns: grid.columns.clone(),
                    column_kinds: grid.column_kinds.clone(),
                    selected_columns: (0..grid.columns.len()).collect(),
                    rows: grid.rows.clone(),
                }))
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
