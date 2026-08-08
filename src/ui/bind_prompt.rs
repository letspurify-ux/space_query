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
use crate::sql_parser_engine::lexical_spans;
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
pub fn collect_bind_params(
    sql: &str,
    db_type: DatabaseType,
    session: &SessionState,
    remembered: &HashMap<String, RememberedValue>,
) -> Vec<BindParam> {
    let declared = declared_bind_names(sql, db_type, session);

    let mut taken: HashSet<String> = declared.clone();
    let mut params: Vec<BindParam> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for name in named_placeholder_names(sql, db_type) {
        if declared.contains(&name) || !seen.insert(name.clone()) {
            continue;
        }
        taken.insert(name.clone());
        params.push(new_param(
            format!(":{name}"),
            name.clone(),
            name,
            remembered,
        ));
    }

    for ordinal in 1..=positional_placeholder_count(sql, db_type) {
        let bind_name = generate_positional_bind_name(ordinal, &mut taken);
        params.push(new_param(
            format!("? {ordinal}"),
            format!("?{ordinal}"),
            bind_name,
            remembered,
        ));
    }

    params
}

fn new_param(
    label: String,
    memo_key: String,
    bind_name: String,
    remembered: &HashMap<String, RememberedValue>,
) -> BindParam {
    let previous = remembered.get(&memo_key);
    BindParam {
        label,
        memo_key,
        bind_name,
        param_type: previous.map(|value| value.param_type).unwrap_or_default(),
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

        if mysql && bytes[idx] == b':' {
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
