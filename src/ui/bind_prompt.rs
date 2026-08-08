//! Which placeholders a statement carries, and what to do with the values the
//! user types for them.
//!
//! SQL copied out of application code carries placeholders the editor has no
//! values for: `WHERE id = :id` or `WHERE id = ?`. Before this module they
//! reached the server as-is and failed — Oracle with "Bind variable :id is not
//! defined", the MySQL family with a syntax error.
//!
//! Two things differ per backend, and both are settled here so the dialog and
//! the execution path stay free of dialect knowledge:
//!
//! * **How a value reaches the server.** Oracle has a real bind pipeline
//!   ([`QueryExecutor::resolve_binds`]), so a prompted value is installed as a
//!   session bind exactly the way `VARIABLE` installs one and the SQL text is
//!   left alone. The MySQL family has no bind path in this app, so the
//!   placeholder is replaced by a literal before the statement is sent.
//! * **What counts as a placeholder.** Oracle named binds go through
//!   [`QueryExecutor::extract_bind_names`], which already knows that `:NEW` in
//!   a `CREATE TRIGGER` body and a colon inside a JSON key are not binds. The
//!   MySQL family uses the parser engine's lexical spans, which know about
//!   backtick identifiers and `#` comments. `?` needs neither: one byte
//!   outside a string or comment is the whole rule.
//!
//! Kept apart from [`crate::ui::bind_prompt_dialog`] so this stays testable
//! without FLTK.

use std::collections::{HashMap, HashSet};

use crate::db::{
    BindDataType, BindVar, DatabaseType, QueryExecutor, ScriptItem, SessionState, SqlValueKind,
    ToolCommand,
};
use crate::sql_parser_engine::{lexical_spans, LexicalKind};
use crate::sql_text::{is_identifier_char, is_identifier_start_byte};
use crate::ui::grid_sql_export::sql_literal_for_value;

/// Prefix for the bind names generated for Oracle `?` placeholders.
const POSITIONAL_BIND_PREFIX: &str = "SQ_P";

/// How a typed value is handed to the server.
///
/// Shorter than [`BindDataType`]: `CLOB` has no meaning for a one-line text
/// field, and `VARIABLE` remains the way to declare one. `Ref Cursor` is here
/// because a PL/SQL OUT cursor has no value to type at all — without it, an
/// undeclared `:rc` could only be answered as text, which fails on the call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BindParamType {
    #[default]
    String,
    Number,
    Date,
    Timestamp,
    RefCursor,
}

impl BindParamType {
    pub const ALL: [BindParamType; 5] = [
        BindParamType::String,
        BindParamType::Number,
        BindParamType::Date,
        BindParamType::Timestamp,
        BindParamType::RefCursor,
    ];

    /// The types worth offering for `db_type`.
    ///
    /// The MySQL family has no ref cursors, and its answers become literals in
    /// the statement text, where a cursor means nothing.
    pub fn offered_for(db_type: DatabaseType) -> &'static [BindParamType] {
        if db_type.is_mysql_or_mariadb() {
            &Self::ALL[..4]
        } else {
            &Self::ALL
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BindParamType::String => "String",
            BindParamType::Number => "Number",
            BindParamType::Date => "Date",
            BindParamType::Timestamp => "Timestamp",
            BindParamType::RefCursor => "Ref Cursor",
        }
    }

    /// Whether an answer of this type is a value the user types.
    pub fn takes_a_value(self) -> bool {
        !matches!(self, BindParamType::RefCursor)
    }

    fn oracle_data_type(self) -> BindDataType {
        match self {
            // The widest VARCHAR2 bind Oracle accepts, so a long pasted value
            // is not silently truncated by the declaration.
            BindParamType::String => BindDataType::Varchar2(4000),
            BindParamType::Number => BindDataType::Number,
            BindParamType::Date => BindDataType::Date,
            BindParamType::Timestamp => BindDataType::Timestamp(6),
            BindParamType::RefCursor => BindDataType::RefCursor,
        }
    }

    fn sql_value_kind(self) -> SqlValueKind {
        match self {
            // `Ref Cursor` is never offered on the family that renders
            // literals, so it can only mean "quote it" here.
            BindParamType::String | BindParamType::RefCursor => SqlValueKind::String,
            BindParamType::Number => SqlValueKind::Number,
            BindParamType::Date | BindParamType::Timestamp => SqlValueKind::Temporal,
        }
    }
}

/// One placeholder the user must supply a value for, and the value they gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindParam {
    /// What the dialog shows in the name column: `:ID` or `? 1`.
    pub label: String,
    /// Key the tab remembers the previous answer under.
    pub memo_key: String,
    /// Bind name used on Oracle. For `?` this is a generated name.
    pub bind_name: String,
    pub param_type: BindParamType,
    pub value: String,
    pub is_null: bool,
}

impl BindParam {
    /// The value this answer stands for, or `None` for SQL NULL.
    ///
    /// An empty box means NULL for every type but `String`: there is no such
    /// thing as an empty number or an empty date, and splicing one would put an
    /// empty literal into the statement. `String` keeps the empty text, which
    /// Oracle treats as NULL anyway and the MySQL family renders as `''` — the
    /// distinction only exists there, and there it is the honest answer. A
    /// `Ref Cursor` names an OUT parameter, so it never carries a value.
    fn effective_value(&self) -> Option<String> {
        if self.is_null || !self.param_type.takes_a_value() {
            return None;
        }
        if self.value.is_empty() && self.param_type != BindParamType::String {
            return None;
        }
        Some(self.value.clone())
    }
}

/// A previously typed answer, replayed into the dialog on the next run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedValue {
    pub param_type: BindParamType,
    pub value: String,
    pub is_null: bool,
}

impl From<&BindParam> for RememberedValue {
    fn from(param: &BindParam) -> Self {
        Self {
            param_type: param.param_type,
            value: param.value.clone(),
            is_null: param.is_null,
        }
    }
}

/// What execution needs after the dialog closes.
#[derive(Clone, Debug)]
pub struct PreparedBinds {
    /// The statement text to run. Unchanged on Oracle unless the text used `?`.
    pub sql: String,
    /// Binds to install in the session before running. Empty on MySQL.
    pub session_binds: Vec<(String, BindVar)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlaceholderKey {
    /// Normalized (upper-case, colon-stripped) bind name.
    Named(String),
    /// 1-based order of appearance across the whole text.
    Positional(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Placeholder {
    start: usize,
    end: usize,
    key: PlaceholderKey,
}

/// Placeholders in `sql` that have no value yet, in the order the dialog shows
/// them: named parameters first, then `?` in order of appearance.
///
/// A name already carried by the session is skipped: it was declared with
/// `VARIABLE`, which is an explicit statement about the bind that a prompt has
/// no business overriding. A name the text itself declares is skipped for the
/// same reason — the `VARIABLE` line simply has not run yet.
/// `catalog` carries the type of the column each placeholder is compared with,
/// keyed by memo key, for the callers that could look one up. It is consulted
/// after a remembered answer and before the syntax rules, which only ever fire
/// where no column is named.
pub fn collect_bind_params(
    sql: &str,
    db_type: DatabaseType,
    session: &SessionState,
    remembered: &HashMap<String, RememberedValue>,
    catalog: &HashMap<String, BindParamType>,
) -> Vec<BindParam> {
    let declared = declared_bind_names(sql, db_type, session);
    let inferred = inferred_param_types(sql, db_type);

    let mut taken: HashSet<String> = declared.clone();
    let mut params: Vec<BindParam> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for name in named_placeholder_names(sql, db_type) {
        if declared.contains(&name) || !seen.insert(name.clone()) {
            continue;
        }
        taken.insert(name.clone());
        let suggested = catalog
            .get(&name)
            .copied()
            .or_else(|| inferred.get(&name).copied());
        params.push(new_param(
            format!(":{name}"),
            name.clone(),
            name,
            remembered,
            suggested,
        ));
    }

    for ordinal in 1..=positional_placeholder_count(sql, db_type) {
        let bind_name = generate_positional_bind_name(ordinal, &mut taken);
        let memo_key = format!("?{ordinal}");
        let suggested = catalog
            .get(&memo_key)
            .copied()
            .or_else(|| inferred.get(&memo_key).copied());
        params.push(new_param(
            format!("? {ordinal}"),
            memo_key,
            bind_name,
            remembered,
            suggested,
        ));
    }

    params
}

/// The type each placeholder's position forces, keyed the way the dialog
/// remembers answers: the bind name for `:x`, `?1` for the first `?`.
fn inferred_param_types(sql: &str, db_type: DatabaseType) -> HashMap<String, BindParamType> {
    let mysql = db_type.is_mysql_or_mariadb();
    let mut inferred = HashMap::new();
    for placeholder in scan_all_placeholders(sql, db_type) {
        let Some(param_type) = inferred_param_type(sql, placeholder.start, mysql) else {
            continue;
        };
        let key = match placeholder.key {
            PlaceholderKey::Named(name) => name,
            PlaceholderKey::Positional(ordinal) => format!("?{ordinal}"),
        };
        // One name can appear twice. The first position that forces a type
        // wins rather than the last, so re-reading the statement top to bottom
        // explains what the dialog opened with.
        inferred.entry(key).or_insert(param_type);
    }
    inferred
}

fn new_param(
    label: String,
    memo_key: String,
    bind_name: String,
    remembered: &HashMap<String, RememberedValue>,
    suggested: Option<BindParamType>,
) -> BindParam {
    let previous = remembered.get(&memo_key);
    BindParam {
        label,
        memo_key,
        bind_name,
        // A previous answer is the user's own decision and outranks the guess.
        param_type: previous
            .map(|value| value.param_type)
            .or(suggested)
            .unwrap_or_default(),
        value: previous
            .map(|value| value.value.clone())
            .unwrap_or_default(),
        is_null: previous.is_some_and(|value| value.is_null),
    }
}

/// Turn the filled-in dialog into the statement to run and the binds to install.
pub fn prepare(sql: &str, db_type: DatabaseType, params: &[BindParam]) -> PreparedBinds {
    let mysql = db_type.is_mysql_or_mariadb();

    let mut by_name: HashMap<&str, &BindParam> = HashMap::new();
    let mut positional: Vec<&BindParam> = Vec::new();
    for param in params {
        if param.memo_key.starts_with('?') {
            positional.push(param);
        } else {
            by_name.insert(param.memo_key.as_str(), param);
        }
    }

    let mut rewritten = sql.to_string();
    // Back to front so an earlier replacement never shifts a later span.
    for placeholder in scan_placeholders(sql, db_type).into_iter().rev() {
        let param = match &placeholder.key {
            PlaceholderKey::Named(name) => {
                // Named placeholders are only rewritten on MySQL; on Oracle
                // they are bound, so the scan does not report them at all.
                by_name.get(name.as_str()).copied()
            }
            PlaceholderKey::Positional(ordinal) => positional.get(ordinal - 1).copied(),
        };
        let Some(param) = param else {
            continue;
        };
        let replacement = if mysql {
            literal_for(param)
        } else {
            format!(":{}", param.bind_name)
        };
        rewritten.replace_range(placeholder.start..placeholder.end, &replacement);
    }

    let session_binds = if mysql {
        Vec::new()
    } else {
        params
            .iter()
            .map(|param| {
                (
                    param.bind_name.clone(),
                    BindVar::from_prompt(
                        param.param_type.oracle_data_type(),
                        param.effective_value(),
                    ),
                )
            })
            .collect()
    };

    PreparedBinds {
        sql: rewritten,
        session_binds,
    }
}

fn literal_for(param: &BindParam) -> String {
    let Some(value) = param.effective_value() else {
        return "NULL".to_string();
    };
    // The MySQL family is the only caller, and both members quote the same way.
    sql_literal_for_value(
        DatabaseType::MySQL,
        param.param_type.sql_value_kind(),
        &value,
    )
}

/// Bind names that must not be prompted for: declared by `VARIABLE` in this
/// text, or already declared in the session.
///
/// A session bind the prompt itself wrote is *not* a declaration — it is last
/// run's answer — so it is asked about again, prefilled from what was typed.
fn declared_bind_names(
    sql: &str,
    db_type: DatabaseType,
    session: &SessionState,
) -> HashSet<String> {
    let mut declared: HashSet<String> = session
        .binds
        .iter()
        .filter(|(_, bind)| !bind.prompted)
        .map(|(name, _)| name.clone())
        .collect();
    if db_type.is_mysql_or_mariadb() {
        // `VARIABLE` is a SQL*Plus command; the MySQL script parser has no
        // equivalent, so there is nothing to collect from the text.
        return declared;
    }
    for item in
        crate::ui::sql_editor::query_text::split_script_items_for_db_type(sql, Some(db_type))
    {
        if let ScriptItem::ToolCommand(ToolCommand::Var { name, .. }) = item {
            declared.insert(SessionState::normalize_name(&name));
        }
    }
    declared
}

/// Named placeholders in `sql`, in order of appearance, normalized.
fn named_placeholder_names(sql: &str, db_type: DatabaseType) -> Vec<String> {
    if db_type.is_mysql_or_mariadb() {
        return scan_placeholders(sql, db_type)
            .into_iter()
            .filter_map(|placeholder| match placeholder.key {
                PlaceholderKey::Named(name) => Some(name),
                PlaceholderKey::Positional(_) => None,
            })
            .collect();
    }

    // Oracle: per statement, because `extract_bind_names` decides whether
    // `:NEW` is a bind by looking at the statement it sits in.
    let mut names = Vec::new();
    for item in
        crate::ui::sql_editor::query_text::split_script_items_for_db_type(sql, Some(db_type))
    {
        if let ScriptItem::Statement(statement) = item {
            names.extend(
                QueryExecutor::extract_bind_names(&statement)
                    .into_iter()
                    .map(|name| SessionState::normalize_name(&name)),
            );
        }
    }
    names
}

fn positional_placeholder_count(sql: &str, db_type: DatabaseType) -> usize {
    scan_placeholders(sql, db_type)
        .into_iter()
        .filter(|placeholder| matches!(placeholder.key, PlaceholderKey::Positional(_)))
        .count()
}

/// Placeholder spans this backend rewrites in the statement text.
///
/// Oracle reports `?` only: its named binds are bound, not substituted, so
/// reporting them here would rewrite text that must stay as the user wrote it.
fn scan_placeholders(sql: &str, db_type: DatabaseType) -> Vec<Placeholder> {
    let mysql = db_type.is_mysql_or_mariadb();
    scan_placeholders_inner(sql, mysql, mysql)
}

/// Every placeholder in `sql`, named ones included on all backends.
///
/// [`scan_placeholders`] deliberately hides Oracle's named binds because it
/// drives a text rewrite. Type inference only reads, so it needs to see them.
fn scan_all_placeholders(sql: &str, db_type: DatabaseType) -> Vec<Placeholder> {
    scan_placeholders_inner(sql, db_type.is_mysql_or_mariadb(), true)
}

fn scan_placeholders_inner(sql: &str, mysql: bool, include_named: bool) -> Vec<Placeholder> {
    let spans = lexical_spans(sql, mysql);
    let bytes = sql.as_bytes();

    let mut found = Vec::new();
    let mut ordinal = 0usize;
    let mut idx = 0usize;
    let mut span_idx = 0usize;

    while idx < bytes.len() {
        while span_idx < spans.len() && spans[span_idx].end <= idx {
            span_idx += 1;
        }
        if let Some(span) = spans.get(span_idx) {
            if span.contains(idx) {
                idx = span.end;
                continue;
            }
        }

        if bytes[idx] == b'?' {
            ordinal += 1;
            found.push(Placeholder {
                start: idx,
                end: idx + 1,
                key: PlaceholderKey::Positional(ordinal),
            });
            idx += 1;
            continue;
        }

        if include_named && bytes[idx] == b':' {
            if let Some(end) = named_placeholder_end(sql, idx + 1) {
                found.push(Placeholder {
                    start: idx,
                    end,
                    key: PlaceholderKey::Named(SessionState::normalize_name(&sql[idx + 1..end])),
                });
                idx = end;
                continue;
            }
        }

        idx += 1;
    }

    found
}

/// The column a placeholder is being measured against, when the statement says
/// which one.
///
/// The dialog cannot know what `:id` is from its name, but the statement
/// usually pairs it with a column, and the catalog knows that column's type.
/// `offset` is where the placeholder sits, which is what lets the caller ask
/// the scope resolver which tables are visible right there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindAnchor {
    /// Table alias or name written before the column, if it was qualified.
    pub qualifier: Option<String>,
    pub column: String,
    /// Byte offset of the placeholder this anchor was found for.
    pub offset: usize,
}

/// A copy of `sql` with every string, quoted identifier and comment replaced by
/// spaces.
///
/// The scans below read backwards over raw text looking for operators and
/// keywords. Blanking first is what stops a `--` comment or a `'WHERE x ='`
/// literal from being read as syntax, without teaching each scan the lexer's
/// rules over again. Byte offsets are preserved, so positions still line up
/// with the original.
fn mask_literals_and_comments(sql: &str, mysql: bool) -> String {
    let mut masked = sql.as_bytes().to_vec();
    for span in lexical_spans(sql, mysql) {
        if matches!(span.kind, LexicalKind::QuotedIdentifier) {
            continue;
        }
        for byte in masked.get_mut(span.start..span.end).unwrap_or(&mut []) {
            *byte = b' ';
        }
    }
    // Every replaced byte was one byte wide, so the result is still UTF-8.
    String::from_utf8(masked).unwrap_or_else(|_| sql.to_string())
}

/// The qualified identifier `text` ends with: `col`, `t.col`, or `"Col"`.
fn trailing_qualified_identifier(text: &str) -> Option<(Option<String>, String)> {
    let column = trailing_identifier(text)?;
    let head = text.get(..text.len() - column.raw_len)?;
    let Some(head) = head.strip_suffix('.') else {
        return Some((None, column.name));
    };
    let qualifier = trailing_identifier(head).map(|part| part.name);
    Some((qualifier, column.name))
}

struct IdentifierPart {
    name: String,
    /// Bytes the identifier occupies in the text it was taken from, quotes
    /// included, so the caller can step past it.
    raw_len: usize,
}

/// The identifier `text` ends with, unquoted.
fn trailing_identifier(text: &str) -> Option<IdentifierPart> {
    for quote in ['"', '`'] {
        if let Some(head) = text.strip_suffix(quote) {
            let open = head.rfind(quote)?;
            let name = head.get(open + 1..)?.to_string();
            if name.is_empty() {
                return None;
            }
            return Some(IdentifierPart {
                raw_len: text.len() - open,
                name,
            });
        }
    }
    let word = trailing_word(text);
    if word.is_empty() || word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(IdentifierPart {
        raw_len: word.len(),
        name: word.to_string(),
    })
}

/// Strip one trailing comparison operator, reporting whether there was one.
fn strip_comparison_operator(text: &str) -> Option<&str> {
    for operator in ["<=", ">=", "<>", "!=", "=", "<", ">"] {
        if let Some(head) = text.strip_suffix(operator) {
            return Some(head.trim_end());
        }
    }
    let word = trailing_word(text);
    if word.eq_ignore_ascii_case("LIKE") {
        return Some(text[..text.len() - word.len()].trim_end());
    }
    None
}

/// The column each placeholder is compared with, keyed the way the dialog
/// remembers answers.
///
/// Only shapes that name a column outright are reported. An expression like
/// `UPPER(name) = :n` is left out rather than guessed at: the catalog has no
/// type for `UPPER(name)`, and reporting `name` would be a different claim than
/// the statement makes.
pub fn bind_anchors(sql: &str, db_type: DatabaseType) -> Vec<(String, BindAnchor)> {
    let mysql = db_type.is_mysql_or_mariadb();
    let masked = mask_literals_and_comments(sql, mysql);
    let mut anchors = Vec::new();
    for placeholder in scan_all_placeholders(sql, db_type) {
        let Some(anchor) = anchor_at(&masked, placeholder.start) else {
            continue;
        };
        let key = match &placeholder.key {
            PlaceholderKey::Named(name) => name.clone(),
            PlaceholderKey::Positional(ordinal) => format!("?{ordinal}"),
        };
        anchors.push((key, anchor));
    }
    anchors
}

fn anchor_at(masked: &str, start: usize) -> Option<BindAnchor> {
    let before = masked.get(..start)?.trim_end();

    // `col = :x`, `col >= :x`, `col LIKE :x`, and `SET col = :x`.
    if let Some(head) = strip_comparison_operator(before) {
        return column_anchor(head, start);
    }

    let last = trailing_word(before);
    // `col BETWEEN :a AND :b` — either end of the range.
    if last.eq_ignore_ascii_case("BETWEEN") {
        return column_anchor(before[..before.len() - last.len()].trim_end(), start);
    }
    if last.eq_ignore_ascii_case("AND") {
        let head = before[..before.len() - last.len()].trim_end();
        let head = skip_one_operand(head)?;
        let between = trailing_word(head);
        if between.eq_ignore_ascii_case("BETWEEN") {
            return column_anchor(head[..head.len() - between.len()].trim_end(), start);
        }
        return None;
    }

    // `col IN (:a, :b)` and `INSERT INTO t (a, b) VALUES (:a, :b)`.
    if before.ends_with('(') || before.ends_with(',') {
        let (open, ordinal) = enclosing_list_position(before)?;
        let head = masked.get(..open)?.trim_end();
        let keyword = trailing_word(head);
        if keyword.eq_ignore_ascii_case("IN") {
            return column_anchor(head[..head.len() - keyword.len()].trim_end(), start);
        }
        if keyword.eq_ignore_ascii_case("VALUES") {
            let column = insert_column_at(masked, head.len() - keyword.len(), ordinal)?;
            return Some(BindAnchor {
                qualifier: None,
                column,
                offset: start,
            });
        }
    }

    None
}

fn column_anchor(head: &str, offset: usize) -> Option<BindAnchor> {
    let (qualifier, column) = trailing_qualified_identifier(head)?;
    Some(BindAnchor {
        qualifier,
        column,
        offset,
    })
}

/// Step back over one operand, so `BETWEEN lo AND :hi` can find `BETWEEN`.
///
/// The low end of a range is often a placeholder itself, so a bare `?` and the
/// colon of a `:name` both count as part of the operand being stepped over.
fn skip_one_operand(text: &str) -> Option<&str> {
    if let Some(head) = text.strip_suffix('?') {
        return Some(head.trim_end());
    }
    let identifier = trailing_identifier(text)?;
    let head = text.get(..text.len() - identifier.raw_len)?.trim_end();
    Some(head.strip_suffix(':').map_or(head, str::trim_end))
}

/// Where the list containing the caret opens, and which item the caret is —
/// 0-based, counting commas at this paren depth.
fn enclosing_list_position(before: &str) -> Option<(usize, usize)> {
    let bytes = before.as_bytes();
    let mut depth = 0usize;
    let mut ordinal = 0usize;
    for index in (0..bytes.len()).rev() {
        match bytes[index] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some((index, ordinal));
                }
                depth -= 1;
            }
            b',' if depth == 0 => ordinal += 1,
            _ => {}
        }
    }
    None
}

/// The `ordinal`-th name in the column list of the `INSERT` that ends at
/// `before_values`.
fn insert_column_at(masked: &str, before_values: usize, ordinal: usize) -> Option<String> {
    let head = masked.get(..before_values)?.trim_end();
    if !head.ends_with(')') {
        return None;
    }
    let open = matching_open_paren(head)?;
    let list = head.get(open + 1..head.len() - 1)?;
    let name = list.split(',').nth(ordinal)?.trim();
    trailing_identifier(name).map(|part| part.name)
}

fn matching_open_paren(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for index in (0..bytes.len()).rev() {
        match bytes[index] {
            b')' => depth += 1,
            b'(' => {
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

/// The prompt type a column of this data type should open as.
///
/// Reads the base type name out of a `type_display` such as `NUMBER(10,2)` or
/// `varchar(64)`, so it works on every backend's spelling. Anything not listed
/// is a `String` — CLOB, RAW, JSON and the rest are all typed as text here, and
/// the `VARIABLE` declaration remains the way to say otherwise.
pub fn param_type_for_data_type(type_display: &str) -> BindParamType {
    let base = type_display
        .split(['(', ' '])
        .next()
        .unwrap_or(type_display)
        .trim()
        .to_ascii_uppercase();
    let rest = type_display.to_ascii_uppercase();

    match base.as_str() {
        "NUMBER" | "NUMERIC" | "DECIMAL" | "DEC" | "FLOAT" | "REAL" | "DOUBLE" | "FIXED"
        | "INT" | "INTEGER" | "SMALLINT" | "TINYINT" | "MEDIUMINT" | "BIGINT" | "BINARY_FLOAT"
        | "BINARY_DOUBLE" => BindParamType::Number,
        // Oracle DATE carries a time, MySQL DATE does not; both are answered in
        // the same `YYYY-MM-DD HH:MM:SS` box, so one type covers them.
        "DATE" => BindParamType::Date,
        // `TIMESTAMP WITH TIME ZONE` and friends all start here, and MySQL
        // spells the same thing DATETIME.
        "TIMESTAMP" | "DATETIME" => BindParamType::Timestamp,
        "REF" if rest.contains("CURSOR") => BindParamType::RefCursor,
        "SYS_REFCURSOR" => BindParamType::RefCursor,
        _ => BindParamType::String,
    }
}

/// The type a placeholder's surroundings force, when they force one.
///
/// Only positions where the default of `String` is a *syntax* error are
/// inferred — a row count, an offset, an OUT cursor. Everything else stays
/// `String`, which is the honest answer for a value whose column this code
/// cannot see: guessing `Number` at `WHERE id = :id` would break the run on a
/// varchar key, and a wrong guess costs more than an unset one.
fn inferred_param_type(sql: &str, start: usize, mysql: bool) -> Option<BindParamType> {
    let before = sql.get(..start)?.trim_end();
    let previous = trailing_word(before);
    let upper = previous.to_ascii_uppercase();

    // `LIMIT :n`, `OFFSET :n` — a quoted value is a parse error in both
    // families, which is the reason the type selector exists at all.
    if matches!(upper.as_str(), "LIMIT" | "OFFSET") {
        return Some(BindParamType::Number);
    }
    // `FETCH FIRST :n ROWS ONLY` / `FETCH NEXT :n ROWS ONLY`.
    if matches!(upper.as_str(), "FIRST" | "NEXT") {
        let head = before.get(..before.len() - previous.len())?.trim_end();
        if trailing_word(head).eq_ignore_ascii_case("FETCH") {
            return Some(BindParamType::Number);
        }
    }
    // `WHERE ROWNUM <= :n`: the comparison operator sits between them.
    if previous.is_empty() {
        let head = before.trim_end_matches(['=', '<', '>', '!']).trim_end();
        if head.len() < before.len() && trailing_word(head).eq_ignore_ascii_case("ROWNUM") {
            return Some(BindParamType::Number);
        }
    }
    // `OPEN :rc FOR ...` names an OUT cursor, which has no value to type.
    if !mysql && upper == "OPEN" {
        return Some(BindParamType::RefCursor);
    }

    None
}

/// The run of identifier characters `text` ends with, empty when it ends with
/// anything else.
fn trailing_word(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_identifier_char(*ch))
        .last()
        .map_or(text.len(), |(index, _)| index);
    &text[start..]
}

/// End of the name starting at `start`, or `None` when no name follows.
///
/// A digit start is allowed so `:1` is recognized, matching what Oracle's own
/// extraction accepts. `:=` stops here because `=` starts no name, which is
/// what keeps a PL/SQL or `SET @a := 1` assignment out of the list.
fn named_placeholder_end(sql: &str, start: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let first = *bytes.get(start)?;
    if !is_identifier_start_byte(first) && !first.is_ascii_digit() {
        return None;
    }
    let end = sql[start..]
        .char_indices()
        .find(|(_, ch)| !is_identifier_char(*ch))
        .map(|(offset, _)| start + offset)
        .unwrap_or(sql.len());
    Some(end)
}

/// A bind name for the `ordinal`-th `?` that collides with nothing already in
/// play — neither a real bind in the text nor an earlier generated name.
fn generate_positional_bind_name(ordinal: usize, taken: &mut HashSet<String>) -> String {
    let mut candidate = format!("{POSITIONAL_BIND_PREFIX}{ordinal}");
    let mut suffix = 1usize;
    while taken.contains(&candidate) {
        candidate = format!("{POSITIONAL_BIND_PREFIX}{ordinal}_{suffix}");
        suffix += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

#[cfg(test)]
mod tests;
