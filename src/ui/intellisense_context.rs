use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::sql_parser_engine::SplitState;
use crate::sql_text;
use crate::ui::sql_depth::{
    apply_paren_token, is_top_level_depth, paren_depths, split_top_level_symbol_groups,
    ParenDepthState,
};
use crate::ui::sql_editor::SqlToken;

/// SQL clause phase within a query at a specific depth level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlPhase {
    Initial,
    WithClause,
    CteColumnList,
    DerivedAliasColumnList,
    ConflictTargetList,
    JoinUsingColumnList,
    RecursiveCteColumnList,
    RecursiveCteGeneratedColumnName,
    HierarchicalGeneratedColumnName,
    SelectList,
    IntoClause,
    DmlSetTargetList,
    InsertColumnList,
    MergeInsertColumnList,
    DmlReturningList,
    SelectIntoTarget,
    FetchIntoTarget,
    ExecuteIntoTarget,
    ReturningIntoTarget,
    UsingBindList,
    FromClause,
    JoinCondition,
    WhereClause,
    GroupByClause,
    HavingClause,
    OrderByClause,
    SetClause,
    LockingColumnList,
    ConnectByClause,
    StartWithClause,
    MatchRecognizeClause,
    ValuesClause,
    UpdateTarget,
    DeleteTarget,
    MergeTarget,
    PivotClause,
    ModelClause,
    /// Cursor is at a position that references an existing column of a single
    /// target table inside a DDL statement, e.g. the parenthesised column list
    /// of `CREATE INDEX ... ON t (...)`, or `ALTER TABLE t DROP/MODIFY/RENAME/
    /// CHANGE/ALTER [COLUMN] ...`. These are column-name positions that DataGrip/
    /// Toad complete with the target table's columns; the DML state machine
    /// otherwise leaves them in a table-target phase.
    DdlColumnList,
}

impl SqlPhase {
    pub fn is_column_context(&self) -> bool {
        matches!(
            self,
            SqlPhase::CteColumnList
                | SqlPhase::DerivedAliasColumnList
                | SqlPhase::ConflictTargetList
                | SqlPhase::JoinUsingColumnList
                | SqlPhase::RecursiveCteColumnList
                | SqlPhase::DmlSetTargetList
                | SqlPhase::InsertColumnList
                | SqlPhase::MergeInsertColumnList
                | SqlPhase::DmlReturningList
                | SqlPhase::SelectList
                | SqlPhase::WhereClause
                | SqlPhase::JoinCondition
                | SqlPhase::GroupByClause
                | SqlPhase::HavingClause
                | SqlPhase::OrderByClause
                | SqlPhase::SetClause
                | SqlPhase::LockingColumnList
                | SqlPhase::ValuesClause
                | SqlPhase::ConnectByClause
                | SqlPhase::StartWithClause
                | SqlPhase::MatchRecognizeClause
                | SqlPhase::PivotClause
                | SqlPhase::ModelClause
                | SqlPhase::DdlColumnList
        )
    }

    pub fn is_table_context(&self) -> bool {
        matches!(
            self,
            SqlPhase::FromClause
                | SqlPhase::IntoClause
                | SqlPhase::UpdateTarget
                | SqlPhase::DeleteTarget
                | SqlPhase::MergeTarget
        )
    }

    pub fn is_variable_context(&self) -> bool {
        matches!(
            self,
            SqlPhase::SelectIntoTarget
                | SqlPhase::FetchIntoTarget
                | SqlPhase::ExecuteIntoTarget
                | SqlPhase::ReturningIntoTarget
        )
    }

    pub fn is_bind_context(&self) -> bool {
        matches!(self, SqlPhase::UsingBindList)
    }

    pub fn is_generated_name_context(&self) -> bool {
        matches!(
            self,
            SqlPhase::RecursiveCteGeneratedColumnName | SqlPhase::HierarchicalGeneratedColumnName
        )
    }
}

/// A table/view reference with optional alias, collected from a query scope.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScopedTableRef {
    pub name: String,
    pub alias: Option<String>,
    pub depth: usize,
    pub is_cte: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TableRefDeclaredAtCursor {
    pub table: ScopedTableRef,
    pub token_start: usize,
}

/// CTE definition parsed from WITH clause.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CteDefinition {
    pub name: String,
    pub depth: usize,
    pub explicit_columns: Vec<String>,
    /// Token range for explicit column list inside `WITH cte(col1, col2) ...`.
    pub explicit_column_range: Option<TokenRange>,
    /// Token range inside `CursorContext.statement_tokens` for the CTE body.
    pub body_range: TokenRange,
}

/// A virtual relation alias with its body token range, for column inference.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SubqueryDefinition {
    pub alias: String,
    pub source_relation: Option<String>,
    pub explicit_columns: Vec<String>,
    pub explicit_column_range: Option<TokenRange>,
    pub body_range: TokenRange,
    pub depth: usize,
}

/// Inclusive-exclusive token range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenRange {
    pub start: usize,
    pub end: usize,
}

impl TokenRange {
    pub fn empty() -> Self {
        Self { start: 0, end: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// Result of deep context analysis at cursor position.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CursorContext {
    /// Full token stream for the normalized statement.
    pub(crate) statement_tokens: Arc<[SqlToken]>,
    /// Number of tokens located before/at cursor in `statement_tokens`.
    pub cursor_token_len: usize,
    /// Innermost query body containing the cursor when inside a CTE/subquery.
    pub(crate) active_query_range: Option<TokenRange>,
    /// Whether `active_query_range` is narrowed by set-operation branch split
    /// (`UNION`/`INTERSECT`/`MINUS`/`EXCEPT`) that should isolate symbol
    /// visibility per branch.
    pub(crate) in_set_operation_query_branch: bool,
    /// Current SQL phase at cursor position
    pub phase: SqlPhase,
    /// Current parenthesis nesting depth (0 = top level)
    pub depth: usize,
    /// All tables visible at cursor position (current scope + parent scopes + CTEs)
    pub tables_in_scope: Vec<ScopedTableRef>,
    /// All tables whose declarations are visible at cursor scope and declared
    /// before the cursor position.
    pub tables_in_scope_before_cursor: Vec<ScopedTableRef>,
    /// Same as `tables_in_scope_before_cursor`, but with declaration position.
    pub(crate) tables_in_scope_before_cursor_with_pos: Vec<TableRefDeclaredAtCursor>,
    /// CTEs defined in WITH clause
    pub ctes: Vec<CteDefinition>,
    /// Subquery aliases with their body tokens for column inference
    pub subqueries: Vec<SubqueryDefinition>,
    /// Preferred relation scope for unqualified column suggestions at cursor.
    pub focused_tables: Vec<String>,
    /// The qualifier before cursor (e.g., "t" in "t.col")
    pub qualifier: Option<String>,
    /// Resolved table names for the qualifier
    pub qualifier_tables: Vec<String>,
    /// The cursor introduces a brand-new DDL name or expects a data type
    /// (`ALTER TABLE t ADD <col> …`, `RENAME … TO <newname>`), where existing
    /// relations/columns/keywords are all irrelevant. Drives completion
    /// suppression, independent of `phase`.
    pub(crate) ddl_new_name_position: bool,
}

/// CTE parsing state machine
#[derive(Debug, Clone, Copy, PartialEq)]
enum CteState {
    Inactive,
    ExpectName,
    AfterName,
    ExpectAs,
    ExpectBody,
    InBody { body_depth: usize },
}

impl CteState {
    fn enter_body(self, body_depth: usize) -> Self {
        if matches!(self, Self::ExpectBody) {
            Self::InBody { body_depth }
        } else {
            self
        }
    }

    fn enter_explicit_column_list(self) -> Self {
        if matches!(self, Self::AfterName) {
            Self::ExpectAs
        } else {
            self
        }
    }

    fn close_parenthesis(self, current_depth: usize) -> Self {
        match self {
            Self::InBody { body_depth } if body_depth == current_depth => Self::Inactive,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WithPlsqlPendingDeclaration {
    starts_body: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WithPlsqlBodyFrameKind {
    Routine,
    Block,
    Case,
    If,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WithPlsqlBodyFrame {
    kind: WithPlsqlBodyFrameKind,
    awaiting_begin: bool,
}

impl WithPlsqlBodyFrame {
    fn routine() -> Self {
        Self {
            kind: WithPlsqlBodyFrameKind::Routine,
            awaiting_begin: true,
        }
    }

    fn nested(kind: WithPlsqlBodyFrameKind) -> Self {
        Self {
            kind,
            awaiting_begin: false,
        }
    }

    fn awaiting_begin(kind: WithPlsqlBodyFrameKind) -> Self {
        Self {
            kind,
            awaiting_begin: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WithPlsqlState {
    None,
    Collecting {
        active_body_frames: Vec<WithPlsqlBodyFrame>,
        pending_routine_declaration: Option<WithPlsqlPendingDeclaration>,
        pending_end: bool,
    },
    AwaitingMainQuery,
}

/// Current completion expectation derived from clause semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    None,
    Table,
    Variable,
    BindValue,
}

impl Expectation {
    fn expect_table(&mut self) {
        *self = Self::Table;
    }

    fn clear(&mut self) {
        *self = Self::None;
    }

    fn is_expect_table(self) -> bool {
        matches!(self, Self::Table)
    }
}

/// Analyze the SQL text from statement start to cursor position.
/// Returns a `CursorContext` describing the phase, depth, and available tables.
///
/// `full_statement` is the complete statement token stream.
/// `cursor_token_len` is the count of tokens before/at cursor.
pub(crate) fn analyze_cursor_context(
    full_statement: &[SqlToken],
    cursor_token_len: usize,
) -> CursorContext {
    analyze_cursor_context_arc(full_statement.to_vec().into(), cursor_token_len)
}

pub(crate) fn analyze_cursor_context_owned(
    full_statement: Vec<SqlToken>,
    cursor_token_len: usize,
) -> CursorContext {
    analyze_cursor_context_arc(full_statement.into(), cursor_token_len)
}

pub(crate) fn analyze_cursor_context_arc(
    statement_tokens: Arc<[SqlToken]>,
    cursor_token_len: usize,
) -> CursorContext {
    let clamped_cursor_token_len = cursor_token_len.min(statement_tokens.len());
    let parse_result = scan_cursor_context(statement_tokens.as_ref(), clamped_cursor_token_len);
    let (active_query_range, in_set_operation_query_branch) = find_active_query_range(
        statement_tokens.as_ref(),
        &parse_result.parsed_ctes,
        &parse_result.parsed_subqueries,
        clamped_cursor_token_len,
    );
    let mut visible_cte_entries = parse_result.parsed_ctes.clone();
    for open_entry in parse_result.cursor_open_ctes {
        if visible_cte_entries.iter().any(|entry| {
            entry.scope_id == open_entry.scope_id
                && entry.body_scope_id == open_entry.body_scope_id
                && entry.cte.name.eq_ignore_ascii_case(&open_entry.cte.name)
        }) {
            continue;
        }
        visible_cte_entries.push(open_entry);
    }
    let table_analysis = filter_scope_entries(
        &parse_result.parsed_tables,
        &parse_result.parsed_subqueries,
        &parse_result.visible_scope_chain,
    );
    let mut tables_in_scope_before_cursor = filter_visible_scope_entries_declared_before_cursor(
        &parse_result.parsed_tables,
        &parse_result.visible_scope_chain,
        clamped_cursor_token_len,
    );
    let mut tables_in_scope_before_cursor_with_pos =
        filter_visible_scope_entries_declared_before_cursor_with_pos(
            &parse_result.parsed_tables,
            &parse_result.visible_scope_chain,
            clamped_cursor_token_len,
        );
    let ctes = filter_visible_ctes(
        &visible_cte_entries,
        &parse_result.visible_cte_scope_chain,
        clamped_cursor_token_len,
    );

    let mut tables_in_scope = table_analysis.tables;
    apply_cte_and_excluded_target_tables(
        &mut tables_in_scope,
        &ctes,
        parse_result.depth,
        parse_result.excluded_target_table.as_deref(),
    );
    apply_cte_and_excluded_target_tables(
        &mut tables_in_scope_before_cursor,
        &ctes,
        parse_result.depth,
        parse_result.excluded_target_table.as_deref(),
    );
    apply_cte_and_excluded_target_tables_with_pos(
        &mut tables_in_scope_before_cursor_with_pos,
        &ctes,
        parse_result.depth,
        parse_result.excluded_target_table.as_deref(),
        clamped_cursor_token_len,
    );

    let mut phase = parse_result.phase;
    let mut focused_tables = parse_result.focused_tables;
    let mut ddl_new_name_position = false;
    if let Some(target) =
        ddl_definition_list_target(statement_tokens.as_ref(), clamped_cursor_token_len)
    {
        // Parenthesised DDL table-definition lists (`CREATE TABLE (...)`,
        // `ALTER TABLE t ADD (...)`, and their `… KEY (...)` / `REFERENCES x (...)`
        // sub-lists). A new column/constraint name suppresses identifiers; a
        // constraint/reference column list resolves to the target table's
        // existing columns. The generic DML machine otherwise leaves these as a
        // table target (offering relations) or a derived-alias list (offering the
        // wrong table's columns).
        match target {
            DdlDefinitionTarget::NewName => {
                ddl_new_name_position = true;
            }
            DdlDefinitionTarget::ExistingColumns(table) => {
                phase = SqlPhase::DdlColumnList;
                focused_tables = vec![table];
            }
        }
    } else if let Some(source) = subquery_alias_column_list_source_at_cursor(
        &table_analysis.subqueries,
        clamped_cursor_token_len,
    ) {
        phase = SqlPhase::DerivedAliasColumnList;
        focused_tables = vec![source];
    } else if let Some(ddl_table) =
        ddl_existing_column_target_table(statement_tokens.as_ref(), clamped_cursor_token_len)
    {
        // DDL column-name positions (CREATE INDEX list, ALTER TABLE column ops)
        // reference the single target table's existing columns.
        phase = SqlPhase::DdlColumnList;
        focused_tables = vec![ddl_table];
    } else if ddl_alter_table_introduces_new_name(
        statement_tokens.as_ref(),
        clamped_cursor_token_len,
    ) || ddl_rename_target_introduces_new_name(
        statement_tokens.as_ref(),
        clamped_cursor_token_len,
    ) {
        // New column/constraint/rename-target name or data-type position: never
        // an existing relation, so suppress identifier suggestions entirely.
        ddl_new_name_position = true;
    }

    CursorContext {
        statement_tokens,
        cursor_token_len: clamped_cursor_token_len,
        active_query_range,
        in_set_operation_query_branch,
        phase,
        depth: parse_result.depth,
        tables_in_scope,
        tables_in_scope_before_cursor,
        tables_in_scope_before_cursor_with_pos,
        ctes,
        subqueries: table_analysis.subqueries,
        focused_tables,
        qualifier: None,
        qualifier_tables: Vec::new(),
        ddl_new_name_position,
    }
}

/// Significant (non-comment) tokens of the statement that contains the cursor,
/// restricted to those strictly before the cursor. The statement is delimited
/// by the nearest preceding top-level `;`.
fn significant_statement_tokens_before_cursor(
    tokens: &[SqlToken],
    cursor_token_len: usize,
) -> Vec<&SqlToken> {
    let end = cursor_token_len.min(tokens.len());
    let mut start = 0usize;
    for (idx, token) in tokens.iter().enumerate().take(end) {
        if let SqlToken::Symbol(sym) = token {
            if sym == ";" {
                start = idx + 1;
            }
        }
    }
    tokens[start..end]
        .iter()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect()
}

/// Reads a dotted identifier name (`schema.table`, `"Schema"."Table"`, …)
/// beginning at `start` in `sig`. Returns the joined name and the index of the
/// first token after the name.
fn read_dotted_name(sig: &[&SqlToken], start: usize) -> Option<(String, usize)> {
    let mut idx = start;
    let mut name = String::new();
    let mut expect_word = true;
    while idx < sig.len() {
        match sig[idx] {
            SqlToken::Word(word) if expect_word => {
                name.push_str(word);
                expect_word = false;
                idx += 1;
            }
            SqlToken::Symbol(sym) if !expect_word && sym == "." => {
                name.push('.');
                expect_word = true;
                idx += 1;
            }
            _ => break,
        }
    }
    if name.is_empty() {
        return None;
    }
    // A trailing dot (`schema.`) leaves the name harmlessly qualified; callers
    // that need a complete name reject it via the following token check.
    Some((name, idx))
}

fn token_is_word(token: &SqlToken, keyword: &str) -> bool {
    matches!(token, SqlToken::Word(word) if word.eq_ignore_ascii_case(keyword))
}

fn token_is_symbol(token: &SqlToken, symbol: &str) -> bool {
    matches!(token, SqlToken::Symbol(sym) if sym == symbol)
}

/// Detects whether the cursor sits at a position that references an existing
/// column of a single DDL target table, returning that target table name.
///
/// Covers the column-name positions of:
///   * `CREATE [UNIQUE|BITMAP] INDEX ... ON t (...)` — anywhere in the list.
///   * `ALTER TABLE t DROP [COLUMN] ...` / `DROP (...)`.
///   * `ALTER TABLE t MODIFY ...` / `MODIFY (...)` (column-name position only).
///   * `ALTER TABLE t RENAME COLUMN ...` (the source name, before `TO`).
///   * `ALTER TABLE t CHANGE ...` (MySQL, the source name).
///   * `ALTER TABLE t ALTER [COLUMN] ...`.
///
/// `ADD`/`RENAME TO`/type positions deliberately return `None`: those introduce
/// new names or expect data types, not existing columns.
fn ddl_existing_column_target_table(
    tokens: &[SqlToken],
    cursor_token_len: usize,
) -> Option<String> {
    let sig = significant_statement_tokens_before_cursor(tokens, cursor_token_len);
    let first = sig.first()?;

    if token_is_word(first, "CREATE") {
        return ddl_create_index_column_target(&sig);
    }
    if token_is_word(first, "ALTER") {
        return ddl_alter_table_column_target(&sig);
    }
    None
}

fn ddl_create_index_column_target(sig: &[&SqlToken]) -> Option<String> {
    if !sig.iter().any(|token| token_is_word(token, "INDEX")) {
        return None;
    }
    // Last top-level ON keyword introduces the indexed table.
    let on_idx = sig.iter().rposition(|token| token_is_word(token, "ON"))?;
    let (table, after_table) = read_dotted_name(sig, on_idx + 1)?;
    // Find the column-list opening paren after the table (skipping modifiers
    // such as `USING btree`).
    let paren_idx = (after_table..sig.len()).find(|&i| token_is_symbol(sig[i], "("))?;
    // Cursor is inside the list only while the paren balance stays positive.
    let mut depth = 0i32;
    for token in &sig[paren_idx..] {
        if token_is_symbol(token, "(") {
            depth += 1;
        } else if token_is_symbol(token, ")") {
            depth -= 1;
        }
    }
    (depth >= 1).then_some(table)
}

/// True when the cursor sits at an `ALTER TABLE … ADD …` or `RENAME … TO …`
/// position that introduces a brand-new name (new column, constraint, or the
/// rename target) or expects a data type. Such positions never reference an
/// existing relation, so the default table-target classification — which would
/// offer existing tables — is wrong there.
///
/// Deliberately conservative so legitimate suggestions are preserved:
///   * positions inside a parenthesised list (e.g. `ADD (col …)`,
///     `… KEY (col)`) keep their existing classification;
///   * the `REFERENCES <table>` target stays a relation reference;
///   * the `ALTER TABLE <name>` and op-keyword (`ALTER TABLE t |`) slots stay
///     table context (the latter cannot be told apart from typing the table
///     name at token granularity).
fn ddl_alter_table_introduces_new_name(tokens: &[SqlToken], cursor_token_len: usize) -> bool {
    let sig = significant_statement_tokens_before_cursor(tokens, cursor_token_len);
    if sig.len() < 2 || !token_is_word(sig[0], "ALTER") || !token_is_word(sig[1], "TABLE") {
        return false;
    }
    let Some((_, after_table)) = read_dotted_name(&sig, 2) else {
        return false;
    };

    // Walk the clause list after the table at the top level (paren depth 0),
    // tracking the governing operation and whether the current action has
    // entered a `REFERENCES <table>` target or passed a `TO` rename target.
    // `REFERENCES`/`TO` are scoped to their action: a top-level `,` (or a new
    // op keyword) starts a fresh action and resets them.
    let mut depth = 0i32;
    let mut op: Option<&str> = None;
    let mut saw_references = false;
    let mut saw_to = false;
    // For MySQL `CHANGE [COLUMN] old_name new_name …`: whether the source column
    // name has been passed, after which the cursor is at the new-name/data-type
    // position rather than the (existing) source column.
    let mut change_source_seen = false;
    let mut action_words: Vec<String> = Vec::new();
    for token in &sig[after_table..] {
        if token_is_symbol(token, "(") {
            depth += 1;
            continue;
        }
        if token_is_symbol(token, ")") {
            depth = (depth - 1).max(0);
            continue;
        }
        if depth != 0 {
            continue;
        }
        if token_is_symbol(token, ",") {
            saw_references = false;
            saw_to = false;
            change_source_seen = false;
            action_words.clear();
            continue;
        }
        if let SqlToken::Word(word) = token {
            match word.to_ascii_uppercase().as_str() {
                kw @ ("ADD" | "DROP" | "MODIFY" | "RENAME" | "CHANGE" | "ALTER") => {
                    op = Some(match kw {
                        "ADD" => "ADD",
                        "RENAME" => "RENAME",
                        "CHANGE" => "CHANGE",
                        _ => "OTHER",
                    });
                    saw_references = false;
                    saw_to = false;
                    change_source_seen = false;
                    action_words.clear();
                }
                "REFERENCES" => {
                    saw_references = true;
                    action_words.push("REFERENCES".to_string());
                }
                "TO" => {
                    saw_to = true;
                    action_words.push("TO".to_string());
                }
                // The optional `COLUMN` keyword precedes the source name.
                "COLUMN" => {}
                _ => {
                    action_words.push(word.to_ascii_uppercase());
                    if op == Some("CHANGE") {
                        change_source_seen = true;
                    }
                }
            }
        }
    }

    // Inside an unclosed parenthesised list: leave the existing classification.
    if depth != 0 {
        return false;
    }

    match op {
        // After ADD every top-level name is a new column/constraint name or a
        // data type — except the `REFERENCES <table>` target.
        Some("ADD") => !saw_references && ddl_alter_table_add_action_is_new_name(&action_words),
        // `RENAME [COLUMN x] TO <newname>` introduces a new name after `TO`.
        Some("RENAME") => saw_to,
        // `CHANGE [COLUMN] old new …`: once the source column is named, the rest
        // of the clause is the new name and its data type.
        Some("CHANGE") => change_source_seen && action_words.len() <= 2,
        _ => false,
    }
}

fn ddl_alter_table_add_action_is_new_name(action_words: &[String]) -> bool {
    match action_words {
        [] => true,
        [word] if word == "CONSTRAINT" => true,
        [word] if word.len() >= 2 && ddl_alter_table_add_structural_keyword_starts_with(word) => {
            false
        }
        [word]
            if matches!(
                word.as_str(),
                "PRIMARY" | "FOREIGN" | "UNIQUE" | "CHECK" | "KEY" | "INDEX"
            ) =>
        {
            false
        }
        [_] => true,
        [first, ..] if first == "CONSTRAINT" => true,
        _ => false,
    }
}

fn ddl_alter_table_add_structural_keyword_starts_with(prefix: &str) -> bool {
    [
        "COLUMN",
        "CONSTRAINT",
        "PRIMARY",
        "UNIQUE",
        "FOREIGN",
        "KEY",
        "INDEX",
        "CHECK",
    ]
    .iter()
    .any(|keyword| keyword.starts_with(prefix))
}

/// True when the cursor sits at the *target* of a `RENAME … TO <newname>`
/// clause — the brand-new name introduced by either a standalone
/// `RENAME [TABLE] old TO new` statement or any `ALTER <object> … RENAME
/// [<part>] TO new` statement. This covers `ALTER TABLE … RENAME TO`,
/// `ALTER TABLE … RENAME COLUMN/CONSTRAINT/PARTITION … TO`, and the sibling
/// object DDLs that share the same `RENAME … TO` tail (`ALTER INDEX`,
/// `ALTER TABLESPACE`, `ALTER VIEW`, `ALTER SEQUENCE`, …). The rename target is
/// never an existing object, so identifier suggestions are suppressed there.
///
/// Deliberately conservative:
///   * only `RENAME`- or `ALTER`-led statements are considered;
///   * the `TO` is bound to its `RENAME` action — it is reset at a top-level
///     `,` and ignored inside parentheses (depth > 0) — so an `ALTER … TO`
///     that is unrelated to a rename, or a `TO` in a parenthesised list, does
///     not trigger;
///   * the source/old-name position (`RENAME COLUMN | TO x`, `RENAME | TO x`)
///     has not yet passed its `TO`, so it stays a real object reference.
fn ddl_rename_target_introduces_new_name(tokens: &[SqlToken], cursor_token_len: usize) -> bool {
    let sig = significant_statement_tokens_before_cursor(tokens, cursor_token_len);
    if sig.len() < 3 || !(token_is_word(sig[0], "RENAME") || token_is_word(sig[0], "ALTER")) {
        return false;
    }

    let mut depth = 0i32;
    let mut in_rename_action = false;
    let mut saw_rename_to = false;
    for token in &sig {
        if token_is_symbol(token, "(") {
            depth += 1;
        } else if token_is_symbol(token, ")") {
            depth = (depth - 1).max(0);
        } else if depth == 0 {
            if token_is_symbol(token, ",") {
                in_rename_action = false;
                saw_rename_to = false;
            } else if token_is_word(token, "RENAME") {
                in_rename_action = true;
                saw_rename_to = false;
            } else if in_rename_action && token_is_word(token, "TO") {
                saw_rename_to = true;
            }
        }
    }

    saw_rename_to
}

/// The kind of position the cursor occupies inside a parenthesised DDL
/// table-definition list (`CREATE TABLE (...)` or `ALTER TABLE … ADD …`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum DdlDefinitionTarget {
    /// A brand-new column or constraint name — never an existing relation or
    /// column, so identifier suggestions are suppressed.
    NewName,
    /// A constraint / `REFERENCES` column-reference list. The named table's
    /// existing columns are the only valid completions (the referenced table for
    /// `REFERENCES x (…)`, otherwise the table being altered/created).
    ExistingColumns(String),
}

/// Classifies the cursor when it sits inside a DDL table-definition list — the
/// parenthesised column/constraint list of `CREATE TABLE …`, or any clause of
/// `ALTER TABLE … ADD …`. Returns `None` for every position outside such a list
/// so the ordinary DML/DDL classification still applies (e.g. `ALTER TABLE t
/// MODIFY …`, the `ALTER TABLE <name>` slot, or a plain `CREATE TABLE t AS
/// SELECT …`).
///
/// Within the list each `(` opens a frame classified from the preceding words:
///   * `PRIMARY KEY (…)` / `UNIQUE (…)` / `FOREIGN KEY (…)` / `KEY (…)` and a
///     `CHECK (…)` expression → existing columns of the table being defined;
///   * `REFERENCES <table> (…)` → existing columns of `<table>`;
///   * the top `ADD (…)` / `CREATE TABLE … (…)` list → a column-definition list,
///     where the start of each entry (right after `(` or `,`) is a new name and
///     a position after the name is a data-type slot left to the caller;
///   * a nested non-constraint paren in that list (a type precision `NUMBER(…)`,
///     a `DEFAULT (…)` expression) is a literal slot, suppressed like a new name.
///
/// A position outside any definition/constraint list is left unclassified.
fn ddl_definition_list_target(
    tokens: &[SqlToken],
    cursor_token_len: usize,
) -> Option<DdlDefinitionTarget> {
    let sig = significant_statement_tokens_before_cursor(tokens, cursor_token_len);
    if sig.len() < 2 {
        return None;
    }

    let is_alter = token_is_word(sig[0], "ALTER") && token_is_word(sig[1], "TABLE");
    let is_create =
        token_is_word(sig[0], "CREATE") && sig.iter().take(6).any(|t| token_is_word(t, "TABLE"));
    if !is_alter && !is_create {
        return None;
    }

    let after_keyword = if is_alter {
        2
    } else {
        sig.iter().position(|t| token_is_word(t, "TABLE"))? + 1
    };
    let (own_table, mut idx) = read_dotted_name(&sig, after_keyword)?;

    // A `CREATE TABLE` definition list is the parenthesised group immediately
    // following the table name. When the next token is anything else (`AS` for
    // `CREATE TABLE t AS SELECT …`, a storage option, …) there is no definition
    // list, so the statement must keep its ordinary classification — otherwise a
    // CTAS subquery/expression paren would be misread as a column list.
    if is_create
        && !sig
            .get(idx)
            .is_some_and(|token| token_is_symbol(token, "("))
    {
        return None;
    }

    #[derive(Clone)]
    enum Frame {
        DefinitionList,
        OwnColumns,
        RefColumns(String),
        Other,
    }

    let mut stack: Vec<Frame> = Vec::new();
    // For `ALTER`, only an `ADD` clause holds new-name / constraint lists; a
    // `CREATE TABLE` body is a definition context throughout.
    let mut op_is_add = is_create;
    let mut create_def_list_opened = false;
    let mut last_word: Option<String> = None;
    let mut references_table: Option<String> = None;
    // Whether a constraint/index keyword has appeared in the current clause entry
    // (since the last `(`/`)`/`,`). It must survive an optional constraint or
    // index name so MySQL's `… KEY idx (col)` / `ADD INDEX idx (col)` still
    // resolve to a column list rather than being read as a table target.
    let mut saw_constraint_kw = false;
    // Whether a `PARTITION BY`/`SUBPARTITION BY` clause is open: its method
    // argument (`RANGE (…)`, `HASH (…)`, `LIST (…)`, `RANGE COLUMNS (…)`, …) is a
    // column/expression list over the table's columns, not a table target.
    let mut saw_partition_by = false;

    while idx < sig.len() {
        let token = sig[idx];
        idx += 1;

        if token_is_symbol(token, "(") {
            let frame = if !op_is_add {
                Frame::Other
            } else if let Some(ref_table) = references_table.clone() {
                Frame::RefColumns(ref_table)
            } else if saw_constraint_kw {
                // `[PRIMARY|FOREIGN] KEY [name] (…)`, `UNIQUE [KEY|INDEX] [name]
                // (…)`, `INDEX [name] (…)` and a `CHECK (…)` expression all
                // reference the table's own columns.
                Frame::OwnColumns
            } else if saw_partition_by && stack.is_empty() {
                // The partition-method column list (`PARTITION BY RANGE (col)`).
                // Reset so the following partition-value parens are not mistaken
                // for further column lists.
                saw_partition_by = false;
                Frame::OwnColumns
            } else if is_create && stack.is_empty() && !create_def_list_opened {
                create_def_list_opened = true;
                Frame::DefinitionList
            } else if is_alter && last_word.as_deref() == Some("ADD") {
                Frame::DefinitionList
            } else {
                Frame::Other
            };
            stack.push(frame);
            last_word = None;
            references_table = None;
            saw_constraint_kw = false;
            continue;
        }
        if token_is_symbol(token, ")") {
            stack.pop();
            last_word = None;
            references_table = None;
            saw_constraint_kw = false;
            continue;
        }
        if token_is_symbol(token, ",") {
            last_word = None;
            references_table = None;
            saw_constraint_kw = false;
            continue;
        }
        if let SqlToken::Word(word) = token {
            let upper = word.to_ascii_uppercase();
            if is_alter && stack.is_empty() {
                match upper.as_str() {
                    "ADD" => op_is_add = true,
                    "MODIFY" | "DROP" | "RENAME" | "CHANGE" | "ALTER" => op_is_add = false,
                    _ => {}
                }
            }
            if matches!(upper.as_str(), "KEY" | "UNIQUE" | "INDEX" | "CHECK") {
                saw_constraint_kw = true;
            }
            if matches!(upper.as_str(), "PARTITION" | "SUBPARTITION") {
                saw_partition_by = true;
            }
            if upper == "REFERENCES" {
                if let Some((name, _)) = read_dotted_name(&sig, idx) {
                    references_table = Some(name);
                }
            }
            last_word = Some(upper);
            continue;
        }
        last_word = None;
    }

    match stack.last() {
        Some(Frame::DefinitionList) => {
            // The start of an entry (right after the list `(` or a `,`) is a new
            // column/constraint name; a later position is a data-type slot the
            // caller resolves on its own.
            let prev = sig.last()?;
            (token_is_symbol(prev, "(") || token_is_symbol(prev, ","))
                .then_some(DdlDefinitionTarget::NewName)
        }
        Some(Frame::OwnColumns) => {
            // `ALTER TABLE t ADD … KEY (col)` references `t`'s existing catalog
            // columns. In `CREATE TABLE` the table does not exist yet, so its
            // constraint columns are the ones being defined in the same
            // statement — not catalog-resolvable here — and offering the catalog
            // would be pure noise, so the slot is left a name position.
            if is_create {
                Some(DdlDefinitionTarget::NewName)
            } else {
                Some(DdlDefinitionTarget::ExistingColumns(own_table))
            }
        }
        Some(Frame::RefColumns(table)) => Some(DdlDefinitionTarget::ExistingColumns(table.clone())),
        Some(Frame::Other) => {
            // A non-constraint nested paren inside a column-definition list is a
            // type precision/size (`NUMBER(…)`, `VARCHAR2(…)`) or a `DEFAULT (…)`
            // expression — a literal position, never an existing relation or
            // column, so suppress rather than leak the catalog. A top-level
            // `Other` (outside any definition list) keeps the default
            // classification.
            stack
                .iter()
                .any(|frame| matches!(frame, Frame::DefinitionList))
                .then_some(DdlDefinitionTarget::NewName)
        }
        None => None,
    }
}

fn ddl_alter_table_column_target(sig: &[&SqlToken]) -> Option<String> {
    if sig.len() < 2 || !token_is_word(sig[1], "TABLE") {
        return None;
    }
    let (table, after_table) = read_dotted_name(sig, 2)?;

    // Walk the clause list after the table, tracking the governing operation
    // keyword seen at the top level (paren depth 0). The op nearest the cursor
    // determines the expected completion.
    let mut depth = 0i32;
    let mut op: Option<&'static str> = None;
    for token in &sig[after_table..] {
        if token_is_symbol(token, "(") {
            depth += 1;
        } else if token_is_symbol(token, ")") {
            depth = (depth - 1).max(0);
        } else if depth == 0 {
            if let SqlToken::Word(word) = token {
                let upper = word.to_ascii_uppercase();
                if matches!(
                    upper.as_str(),
                    "ADD" | "DROP" | "MODIFY" | "RENAME" | "CHANGE" | "ALTER"
                ) {
                    op = Some(match upper.as_str() {
                        "ADD" => "ADD",
                        "DROP" => "DROP",
                        "MODIFY" => "MODIFY",
                        "RENAME" => "RENAME",
                        "CHANGE" => "CHANGE",
                        _ => "ALTER",
                    });
                }
            }
        }
    }

    let prev = sig.last()?;
    let prev_is_word = |kw: &str| token_is_word(prev, kw);
    let prev_is_entry_start =
        prev_is_word("COLUMN") || token_is_symbol(prev, "(") || token_is_symbol(prev, ",");

    let at_existing_column = match op? {
        // New column name / data type position — never an existing column.
        "ADD" => false,
        "DROP" => prev_is_word("DROP") || prev_is_entry_start,
        // `MODIFY [COLUMN] col …` (the optional `COLUMN` keyword is MySQL's) and
        // the Oracle parenthesised `MODIFY (col …, col …)` list both name an
        // existing column right after the op keyword, the optional `COLUMN`, the
        // `(`, or a `,`.
        "MODIFY" => {
            prev_is_word("MODIFY")
                || prev_is_word("COLUMN")
                || token_is_symbol(prev, "(")
                || token_is_symbol(prev, ",")
        }
        "ALTER" => prev_is_word("ALTER") || prev_is_word("COLUMN"),
        // MySQL `CHANGE [COLUMN] old new TYPE`: only the source name references a
        // column (right after `CHANGE` or its optional `COLUMN` keyword).
        "CHANGE" => prev_is_word("CHANGE") || prev_is_word("COLUMN"),
        // `RENAME COLUMN old TO new`: only the source name; `RENAME TO t` is a
        // new table name.
        "RENAME" => prev_is_word("COLUMN"),
        _ => false,
    };

    at_existing_column.then_some(table)
}

fn subquery_alias_column_list_source_at_cursor(
    subqueries: &[SubqueryDefinition],
    cursor_token_len: usize,
) -> Option<String> {
    subqueries.iter().find_map(|subquery| {
        subquery.explicit_column_range.and_then(|range| {
            (cursor_token_len >= range.start && cursor_token_len <= range.end).then(|| {
                subquery
                    .source_relation
                    .clone()
                    .unwrap_or_else(|| subquery.alias.clone())
            })
        })
    })
}

/// Returns true for functions whose syntax includes a FROM keyword as part of
/// the function call rather than a SQL clause (e.g. `EXTRACT(YEAR FROM ...)`,
/// `TRIM(LEADING '0' FROM ...)`, `SUBSTRING(col FROM ...)`).
fn is_from_consuming_function(name: &str) -> bool {
    sql_text::is_from_consuming_function(name)
}

/// FROM-clause table functions that may reference left-side row source aliases
/// without an explicit APPLY/LATERAL modifier.
fn is_implicitly_lateral_table_function(name: &str) -> bool {
    matches!(name, "JSON_TABLE" | "XMLTABLE" | "UNNEST" | "TABLE")
}

fn relation_has_explicit_output_columns(tokens: &[SqlToken]) -> bool {
    !extract_table_function_columns(tokens).is_empty()
}

fn relation_uses_virtual_alias_scope(table_name: &str, relation_body_tokens: &[SqlToken]) -> bool {
    relation_function_name_hint(table_name)
        .as_deref()
        .is_some_and(is_implicitly_lateral_table_function)
        || relation_has_explicit_output_columns(relation_body_tokens)
}

fn is_merge_action_context(
    statement_kind: StatementKind,
    current_phase: SqlPhase,
    last_word: Option<&str>,
) -> bool {
    matches!(statement_kind, StatementKind::Merge)
        && (matches!(current_phase, SqlPhase::JoinCondition) || matches!(last_word, Some("THEN")))
}

fn is_merge_delete_where_action(
    tokens: &[SqlToken],
    idx: usize,
    statement_kind: StatementKind,
    current_phase: SqlPhase,
) -> bool {
    matches!(statement_kind, StatementKind::Merge)
        && matches!(current_phase, SqlPhase::SetClause)
        && matches!(next_word_upper(tokens, idx + 1), Some((next, _)) if next == "WHERE")
}

fn relation_function_name_hint(table_name: &str) -> Option<String> {
    let name_without_dblink = strip_unquoted_dblink_suffix(table_name);
    split_identifier_parts_for_lookup(name_without_dblink)
        .into_iter()
        .next_back()
        .map(|name| strip_identifier_quotes(&name))
        .map(|name| name.to_ascii_uppercase())
}

fn strip_unquoted_dblink_suffix(value: &str) -> &str {
    let mut chars = value.char_indices().peekable();
    let mut active_quote = None;

    while let Some((idx, ch)) = chars.next() {
        if let Some(delimiter) = active_quote {
            if ch == delimiter {
                if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                    chars.next();
                } else {
                    active_quote = None;
                }
            }
            continue;
        }

        match ch {
            '"' | '`' => active_quote = Some(ch),
            '[' => active_quote = Some(']'),
            '@' => return value.get(..idx).unwrap_or(value),
            _ => {}
        }
    }

    value
}

fn is_table_target_statement_keyword(word: &str) -> bool {
    matches!(
        word,
        "ALTER"
            | "DROP"
            | "LOCK"
            | "TRUNCATE"
            | "FLASHBACK"
            | "RENAME"
            | "ANALYZE"
            | "OPTIMIZE"
            | "CHECK"
            | "REPAIR"
    )
}

fn is_comment_on_target(tokens: &[SqlToken], idx: usize, last_word: Option<&str>) -> bool {
    if !matches!(last_word, Some("ON")) {
        return false;
    }

    let mut saw_on_keyword = false;
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        match tokens.get(scan_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Word(word)) => {
                if !saw_on_keyword && word.eq_ignore_ascii_case("ON") {
                    saw_on_keyword = true;
                    continue;
                }
                return saw_on_keyword && word.eq_ignore_ascii_case("COMMENT");
            }
            _ => return false,
        }
    }

    false
}

fn is_comment_on_qualified_view_target(
    tokens: &[SqlToken],
    idx: usize,
    last_word: Option<&str>,
) -> bool {
    let qualifier = match last_word {
        Some(prev) if prev.eq_ignore_ascii_case("MATERIALIZED") => "MATERIALIZED",
        Some(prev) if prev.eq_ignore_ascii_case("EDITIONING") => "EDITIONING",
        _ => return false,
    };

    if idx == 0 {
        return false;
    }

    let mut significant_words = Vec::with_capacity(3);
    let mut scan_idx = idx;
    while scan_idx > 0 && significant_words.len() < 3 {
        scan_idx -= 1;
        match tokens.get(scan_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Word(word)) => significant_words.push(word.to_ascii_uppercase()),
            _ => break,
        }
    }

    matches!(
        significant_words.as_slice(),
        [first, second, third] if first == qualifier && second == "ON" && third == "COMMENT"
    )
}
fn is_create_on_table_target(tokens: &[SqlToken], idx: usize) -> bool {
    let mut scan_idx = idx;
    let mut saw_create_keyword = false;
    let mut saw_object_keyword = false;

    while scan_idx > 0 {
        scan_idx -= 1;
        match tokens.get(scan_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Symbol(sym)) if sym == ";" => break,
            Some(SqlToken::Word(word)) => {
                let upper = word.to_ascii_uppercase();
                if upper == "CREATE" {
                    saw_create_keyword = true;
                    break;
                }

                if matches!(upper.as_str(), "INDEX" | "TRIGGER") {
                    saw_object_keyword = true;
                }

                // CREATE INDEX / CREATE TRIGGER statements often include
                // additional modifiers before the ON-target table name
                // (e.g. `CREATE UNIQUE INDEX ... ON`,
                // `CREATE OR REPLACE TRIGGER ... ON`).
                // Keep scanning until statement start instead of treating
                // intermediate keywords as hard failures.
            }
            _ => {}
        }
    }

    saw_create_keyword && saw_object_keyword
}

fn is_create_table_target(tokens: &[SqlToken], idx: usize) -> bool {
    let mut scan_idx = idx;
    let mut saw_create_keyword = false;

    while scan_idx > 0 {
        scan_idx -= 1;
        match tokens.get(scan_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Symbol(sym)) if sym == ";" => break,
            Some(SqlToken::Word(word)) => {
                let upper = word.to_ascii_uppercase();
                if upper == "CREATE" {
                    saw_create_keyword = true;
                    break;
                }

                if matches!(
                    upper.as_str(),
                    "GLOBAL" | "LOCAL" | "TEMP" | "TEMPORARY" | "UNLOGGED" | "TRANSIENT"
                ) {
                    continue;
                }

                return false;
            }
            _ => return false,
        }
    }

    saw_create_keyword
}

fn is_with_plsql_declaration_keyword(keyword: &str) -> bool {
    sql_text::is_with_plsql_declaration_keyword(keyword)
}

fn with_starts_non_plsql_option(tokens: &[SqlToken], with_idx: usize) -> bool {
    next_word_upper(tokens, with_idx + 1)
        .is_some_and(|(keyword, _)| sql_text::is_with_non_plsql_clause_keyword(&keyword))
}

fn with_parenthesized_clause_looks_like_cte_column_list(
    tokens: &[SqlToken],
    range: TokenRange,
) -> bool {
    let body_tokens = token_range_slice(tokens, range);
    let body_depths = paren_depths(body_tokens);

    for (body_idx, token) in body_tokens.iter().enumerate() {
        if !is_top_level_depth(&body_depths, body_idx) {
            continue;
        }

        match token {
            SqlToken::Comment(_) => {}
            SqlToken::Symbol(sym) if sym == "," => {}
            SqlToken::Word(word)
                if is_identifier_word_token(word)
                    && !sql_text::ORACLE_SQL_KEYWORDS_SET
                        .contains(word.to_ascii_uppercase().as_str()) => {}
            _ => return false,
        }
    }

    true
}

fn with_starts_parenthesized_query_head_clause(tokens: &[SqlToken], with_idx: usize) -> bool {
    let Some((_head_name, head_name_idx)) = next_word_upper(tokens, with_idx + 1) else {
        return false;
    };

    let open_paren_idx = skip_comment_tokens(tokens, head_name_idx + 1);
    if !matches!(tokens.get(open_paren_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        return false;
    }

    let Some((clause_range, after_clause_idx)) =
        extract_parenthesized_range(tokens, open_paren_idx)
    else {
        return false;
    };

    if with_parenthesized_clause_looks_like_cte_column_list(tokens, clause_range) {
        return false;
    }

    next_word_upper(tokens, after_clause_idx)
        .is_some_and(|(keyword, _)| sql_text::is_with_main_query_keyword(&keyword))
}

fn with_starts_non_cte_query_head(tokens: &[SqlToken], with_idx: usize) -> bool {
    next_word_upper(tokens, with_idx + 1)
        .is_some_and(|(keyword, _)| sql_text::is_with_non_cte_query_head_keyword(&keyword))
        || with_starts_parenthesized_query_head_clause(tokens, with_idx)
}

fn should_enter_with_clause(
    tokens: &[SqlToken],
    with_idx: usize,
    current_phase: SqlPhase,
    current_statement_kind: StatementKind,
    relation_state: Expectation,
    depth: usize,
    last_word: Option<&str>,
    allows_leading_query_expression: bool,
) -> bool {
    if with_starts_non_plsql_option(tokens, with_idx) {
        return false;
    }

    if with_starts_non_cte_query_head(tokens, with_idx) {
        return false;
    }

    if matches!(current_phase, SqlPhase::Initial) {
        return true;
    }

    // `WITH FUNCTION/PROCEDURE/...; WITH cte AS (...) SELECT ...` re-enters
    // the main query head after declaration mode has already put the parser in
    // WITH-clause waiting state.
    if matches!(current_phase, SqlPhase::WithClause) && last_word.is_none() {
        return true;
    }

    if matches!(current_phase, SqlPhase::IntoClause)
        && !relation_state.is_expect_table()
        && matches!(current_statement_kind, StatementKind::Insert)
    {
        return true;
    }

    // Preserve hierarchical-query `START WITH` semantics.
    if matches!(last_word, Some(prev) if prev.eq_ignore_ascii_case("START")) {
        return false;
    }
    // Nested subqueries can inherit a non-Initial parent phase (e.g. WHERE),
    // but a leading WITH right after `(` still starts a query scope.
    depth > 0 && last_word.is_none() && allows_leading_query_expression
}

fn is_statement_keyword_suppressed_in_expression_phase(phase: SqlPhase) -> bool {
    matches!(
        phase,
        SqlPhase::SelectList
            | SqlPhase::JoinCondition
            | SqlPhase::WhereClause
            | SqlPhase::GroupByClause
            | SqlPhase::HavingClause
            | SqlPhase::OrderByClause
            | SqlPhase::ConnectByClause
            | SqlPhase::StartWithClause
            | SqlPhase::MatchRecognizeClause
            | SqlPhase::ValuesClause
            | SqlPhase::PivotClause
            | SqlPhase::ModelClause
            | SqlPhase::SetClause
    )
}

fn find_order_by_keyword(tokens: &[SqlToken], start_idx: usize) -> Option<usize> {
    let (next_keyword, next_idx) = next_word_upper(tokens, start_idx)?;
    if next_keyword == "BY" {
        return Some(next_idx);
    }
    if next_keyword == "SIBLINGS" {
        let (tail_keyword, tail_idx) = next_word_upper(tokens, next_idx + 1)?;
        if tail_keyword == "BY" {
            return Some(tail_idx);
        }
    }
    None
}

fn previous_word_upper(tokens: &[SqlToken], start_idx: usize) -> Option<(String, usize)> {
    let mut idx = start_idx;
    while idx > 0 {
        idx -= 1;
        match tokens.get(idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Word(word)) => return Some((word.to_ascii_uppercase(), idx)),
            _ => return None,
        }
    }
    None
}

fn is_log_errors_into_clause(tokens: &[SqlToken], into_idx: usize) -> bool {
    let Some((prev_word, prev_idx)) = previous_word_upper(tokens, into_idx) else {
        return false;
    };
    if prev_word != "ERRORS" {
        return false;
    }

    matches!(
        previous_word_upper(tokens, prev_idx),
        Some((word, _)) if word == "LOG"
    )
}

fn is_log_errors_table_target(tokens: &[SqlToken], table_idx: usize) -> bool {
    let Some((prev_word, prev_idx)) = prev_word_upper(tokens, table_idx) else {
        return false;
    };
    if prev_word != "INTO" {
        return false;
    }

    let Some((second_prev_word, second_prev_idx)) = prev_word_upper(tokens, prev_idx) else {
        return false;
    };
    if second_prev_word != "ERRORS" {
        return false;
    }

    prev_word_upper(tokens, second_prev_idx)
        .is_some_and(|(third_prev_word, _)| third_prev_word == "LOG")
}

fn is_locking_for_clause(tokens: &[SqlToken], start_idx: usize) -> bool {
    let Some((first_keyword, first_idx)) = next_word_upper(tokens, start_idx) else {
        return false;
    };

    if matches!(first_keyword.as_str(), "UPDATE" | "SHARE") {
        return true;
    }

    if first_keyword == "NO" {
        return matches!(
            next_word_upper(tokens, first_idx + 1),
            Some((next_keyword, _)) if next_keyword == "KEY"
        ) && matches!(
            next_word_upper(tokens, first_idx + 2),
            Some((tail_keyword, _)) if tail_keyword == "UPDATE"
        );
    }

    if first_keyword == "KEY" {
        return matches!(
            next_word_upper(tokens, first_idx + 1),
            Some((tail_keyword, _)) if tail_keyword == "SHARE"
        );
    }

    false
}

fn is_multiset_set_operator(tokens: &[SqlToken], idx: usize) -> bool {
    matches!(
        previous_word_upper(tokens, idx),
        Some((prev, _)) if prev == "MULTISET"
    )
}

fn locking_for_clause_has_of_target(tokens: &[SqlToken], start_idx: usize) -> bool {
    let Some((first_keyword, first_idx)) = next_word_upper(tokens, start_idx) else {
        return false;
    };

    let after_locking_idx = match first_keyword.as_str() {
        "UPDATE" | "SHARE" => first_idx + 1,
        "NO" => {
            let Some((second_keyword, second_idx)) = next_word_upper(tokens, first_idx + 1) else {
                return false;
            };
            if second_keyword != "KEY" {
                return false;
            }

            let Some((third_keyword, third_idx)) = next_word_upper(tokens, second_idx + 1) else {
                return false;
            };
            if third_keyword != "UPDATE" {
                return false;
            }
            third_idx + 1
        }
        "KEY" => {
            let Some((second_keyword, second_idx)) = next_word_upper(tokens, first_idx + 1) else {
                return false;
            };
            if second_keyword != "SHARE" {
                return false;
            }
            second_idx + 1
        }
        _ => return false,
    };

    matches!(
        next_word_upper(tokens, after_locking_idx),
        Some((keyword, _)) if keyword == "OF"
    )
}

fn is_recursive_cte_search_by_keyword(tokens: &[SqlToken], by_idx: usize) -> bool {
    previous_word_chain_matches(tokens, by_idx, &["FIRST", "DEPTH", "SEARCH"])
        || previous_word_chain_matches(tokens, by_idx, &["FIRST", "BREADTH", "SEARCH"])
}

fn is_read_consistency_for_clause(tokens: &[SqlToken], start_idx: usize) -> bool {
    let Some((first_keyword, first_idx)) = next_word_upper(tokens, start_idx) else {
        return false;
    };

    if first_keyword != "READ" {
        return false;
    }

    matches!(
        next_word_upper(tokens, first_idx + 1),
        Some((second_keyword, _)) if second_keyword == "ONLY" || second_keyword == "WRITE"
    )
}

fn is_post_query_for_clause(tokens: &[SqlToken], start_idx: usize) -> bool {
    let Some((first_keyword, _)) = next_word_upper(tokens, start_idx) else {
        return false;
    };

    matches!(first_keyword.as_str(), "JSON" | "XML" | "BROWSE")
}

fn is_mysql_lock_in_share_mode_clause(tokens: &[SqlToken], start_idx: usize) -> bool {
    let Some((first_keyword, first_idx)) = next_word_upper(tokens, start_idx) else {
        return false;
    };
    if first_keyword != "IN" {
        return false;
    }

    let Some((second_keyword, second_idx)) = next_word_upper(tokens, first_idx + 1) else {
        return false;
    };
    if second_keyword != "SHARE" {
        return false;
    }

    matches!(
        next_word_upper(tokens, second_idx + 1),
        Some((third_keyword, _)) if third_keyword == "MODE"
    )
}

fn is_post_query_lock_clause(
    tokens: &[SqlToken],
    lock_idx: usize,
    current_phase: SqlPhase,
) -> bool {
    if !matches!(
        current_phase,
        SqlPhase::FromClause
            | SqlPhase::WhereClause
            | SqlPhase::GroupByClause
            | SqlPhase::HavingClause
            | SqlPhase::OrderByClause
    ) {
        return false;
    }

    if is_mysql_lock_in_share_mode_clause(tokens, lock_idx + 1) {
        return true;
    }

    // Keep incomplete `... LOCK` / `... LOCK IN` edits in post-query context
    // so MySQL/MariaDB lock modifiers are not misclassified as `LOCK TABLE`.
    next_word_upper(tokens, lock_idx + 1).is_none_or(|(next_keyword, _)| next_keyword == "IN")
}

fn is_mysql_lock_table_mode_keyword(keyword: &str) -> bool {
    matches!(keyword, "READ" | "WRITE" | "LOCAL" | "LOW_PRIORITY")
}

fn is_insert_target_modifier_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "LOW_PRIORITY" | "HIGH_PRIORITY" | "DELAYED" | "IGNORE"
    )
}

fn should_promote_insert_modifier_to_target_context(
    statement_kind: StatementKind,
    current_phase: SqlPhase,
    last_word: Option<&str>,
) -> bool {
    matches!(statement_kind, StatementKind::Insert)
        && matches!(current_phase, SqlPhase::Initial)
        && matches!(
            last_word,
            Some(previous)
                if previous == "INSERT"
                    || is_insert_target_modifier_keyword(previous)
        )
}

fn should_keep_waiting_for_insert_target_after_modifier(
    statement_kind: StatementKind,
    current_phase: SqlPhase,
    relation_state: Expectation,
    keyword: &str,
) -> bool {
    matches!(statement_kind, StatementKind::Insert)
        && matches!(current_phase, SqlPhase::IntoClause)
        && relation_state.is_expect_table()
        && is_insert_target_modifier_keyword(keyword)
}

fn has_table_target_statement_keyword_before_table(
    tokens: &[SqlToken],
    table_idx: usize,
    last_word: Option<&str>,
) -> bool {
    last_word.is_some_and(is_table_target_statement_keyword)
        || previous_word_upper(tokens, table_idx)
            .is_some_and(|(keyword, _)| is_table_target_statement_keyword(&keyword))
}

fn has_lock_keyword_before_table(
    tokens: &[SqlToken],
    table_idx: usize,
    last_word: Option<&str>,
) -> bool {
    matches!(last_word, Some("LOCK"))
        || previous_word_upper(tokens, table_idx).is_some_and(|(keyword, _)| keyword == "LOCK")
}

fn sanitize_lock_table_alias(
    tokens: &[SqlToken],
    alias_start: usize,
    alias: Option<String>,
    alias_end: usize,
    statement_kind: StatementKind,
    lock_table_list_active: bool,
) -> (Option<String>, usize) {
    if !matches!(statement_kind, StatementKind::Lock) || !lock_table_list_active {
        return (alias, alias_end);
    }

    let Some(alias_name) = alias else {
        return (None, alias_end);
    };

    let upper = alias_name.to_ascii_uppercase();
    if !is_mysql_lock_table_mode_keyword(upper.as_str()) {
        return (Some(alias_name), alias_end);
    }

    // In `LOCK TABLES t READ`, lock-mode tokens are statement modifiers, not
    // aliases. Keep quoted aliases available (`AS `read``) by checking the raw token.
    let mut alias_token_idx = alias_end.min(tokens.len());
    let mut alias_is_quoted = false;
    while alias_token_idx > alias_start {
        alias_token_idx -= 1;
        match tokens.get(alias_token_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Word(raw_alias)) => {
                alias_is_quoted = is_quoted_identifier(raw_alias);
                break;
            }
            _ => break,
        }
    }
    if alias_is_quoted {
        (Some(alias_name), alias_end)
    } else {
        (None, alias_start)
    }
}

pub(crate) fn is_query_expression_start(tokens: &[SqlToken], start_idx: usize) -> bool {
    let mut idx = skip_comment_tokens(tokens, start_idx);

    while matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        idx = skip_comment_tokens(tokens, idx + 1);
    }

    matches!(
        tokens.get(idx),
        Some(SqlToken::Word(word))
            if matches!(
                word.to_ascii_uppercase().as_str(),
                "SELECT"
                    | "WITH"
                    | "VALUES"
                    | "TABLE"
                    | "ONLY"
                    | "CONTAINERS"
                    | "SHARDS"
                    | "ROWS"
            )
    )
}

#[derive(Debug, Clone)]
struct ParsedTableEntry {
    table: ScopedTableRef,
    scope_id: usize,
    token_start: usize,
    origin: ParsedTableEntryOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedTableEntryOrigin {
    Relation,
    DerivedAlias,
}

#[derive(Debug, Clone)]
struct ParsedSubqueryEntry {
    subquery: SubqueryDefinition,
    scope_id: usize,
}

#[derive(Debug, Clone)]
struct ParsedCteEntry {
    cte: CteDefinition,
    scope_id: usize,
    visible_from_token: usize,
    body_scope_id: usize,
    self_visible_from_token: Option<usize>,
}

#[derive(Debug, Clone)]
struct PendingCteHeader {
    name: String,
    depth: usize,
    explicit_columns: Vec<String>,
    explicit_column_range: Option<TokenRange>,
    scope_id: usize,
    visible_from_token: usize,
}

#[derive(Debug, Clone)]
struct OpenCteDefinition {
    header: PendingCteHeader,
    body_depth: usize,
    body_start: usize,
    body_scope_id: usize,
}

#[derive(Debug, Clone)]
struct CursorScanResult {
    phase: SqlPhase,
    depth: usize,
    visible_scope_chain: Vec<usize>,
    visible_cte_scope_chain: Vec<usize>,
    parsed_tables: Vec<ParsedTableEntry>,
    parsed_subqueries: Vec<ParsedSubqueryEntry>,
    parsed_ctes: Vec<ParsedCteEntry>,
    cursor_open_ctes: Vec<ParsedCteEntry>,
    focused_tables: Vec<String>,
    excluded_target_table: Option<String>,
}

/// Build visible scope chain from current scope to root.
fn build_visible_scope_chain(
    scope_stack: &[usize],
    visible_parent: &HashMap<usize, Option<usize>>,
) -> Vec<usize> {
    let mut visible_scope_chain = Vec::new();
    let mut scope_id = *scope_stack.last().unwrap_or(&0);
    visible_scope_chain.push(scope_id);
    while let Some(Some(parent_id)) = visible_parent.get(&scope_id) {
        visible_scope_chain.push(*parent_id);
        scope_id = *parent_id;
    }
    visible_scope_chain.reverse();
    visible_scope_chain
}

fn scope_is_within_subtree(
    scope_id: usize,
    ancestor_scope_id: usize,
    scope_parent: &HashMap<usize, Option<usize>>,
) -> bool {
    let mut current_scope = Some(scope_id);
    while let Some(active_scope) = current_scope {
        if active_scope == ancestor_scope_id {
            return true;
        }
        current_scope = scope_parent.get(&active_scope).copied().flatten();
    }
    false
}

fn skip_set_operator_suffix(tokens: &[SqlToken], start_idx: usize) -> usize {
    let mut idx = skip_comment_tokens(tokens, start_idx);

    loop {
        let Some((keyword, keyword_idx)) = next_word_upper(tokens, idx) else {
            return idx;
        };

        if matches!(keyword.as_str(), "ALL" | "DISTINCT") {
            idx = skip_comment_tokens(tokens, keyword_idx + 1);
            continue;
        }

        return idx;
    }
}

fn find_top_level_set_operator_operands(tokens: &[SqlToken], range: TokenRange) -> Vec<TokenRange> {
    let body_tokens = token_range_slice(tokens, range);
    let body_depths = paren_depths(body_tokens);
    let mut operands = Vec::new();
    let mut operand_start = range.start;
    let mut saw_set_operator = false;

    for (local_idx, token) in body_tokens.iter().enumerate() {
        if !is_top_level_depth(&body_depths, local_idx) {
            continue;
        }

        let SqlToken::Word(word) = token else {
            continue;
        };
        let upper = word.to_ascii_uppercase();
        if matches!(upper.as_str(), "UNION" | "INTERSECT" | "EXCEPT" | "MINUS")
            && !is_multiset_set_operator(body_tokens, local_idx)
        {
            let operator_idx = range.start + local_idx;
            if operand_start < operator_idx {
                operands.push(TokenRange {
                    start: operand_start,
                    end: operator_idx,
                });
            }
            operand_start = skip_set_operator_suffix(tokens, operator_idx + 1).min(range.end);
            saw_set_operator = true;
        }
    }

    if saw_set_operator && operand_start < range.end {
        operands.push(TokenRange {
            start: operand_start,
            end: range.end,
        });
    }

    operands
}

fn update_cte_body_self_visibility(
    parsed_ctes: &mut [ParsedCteEntry],
    parsed_tables: &[ParsedTableEntry],
    scope_parent: &HashMap<usize, Option<usize>>,
    tokens: &[SqlToken],
) {
    for entry in parsed_ctes {
        let operand_ranges = find_top_level_set_operator_operands(tokens, entry.cte.body_range);
        if operand_ranges.len() < 2 {
            entry.self_visible_from_token = None;
            continue;
        }

        entry.self_visible_from_token = operand_ranges.iter().skip(1).find_map(|operand_range| {
            parsed_tables
                .iter()
                .any(|table_entry| {
                    table_entry.origin == ParsedTableEntryOrigin::Relation
                        && table_entry.token_start >= operand_range.start
                        && table_entry.token_start < operand_range.end
                        && table_entry.table.name.eq_ignore_ascii_case(&entry.cte.name)
                        && scope_is_within_subtree(
                            table_entry.scope_id,
                            entry.body_scope_id,
                            scope_parent,
                        )
                })
                .then_some(operand_range.start)
        });
    }
}

fn nearest_target_table(depth_frames: &[ParserDepthFrame], depth: usize) -> Option<String> {
    depth_frames[..=depth.min(depth_frames.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|frame| frame.current_target_table.clone())
}

fn nearest_cte_name(depth_frames: &[ParserDepthFrame], depth: usize) -> Option<String> {
    depth_frames[..=depth.min(depth_frames.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|frame| frame.current_cte_name.clone())
}

fn nearest_alias_column_list_source(
    depth_frames: &[ParserDepthFrame],
    depth: usize,
) -> Option<String> {
    depth_frames[..=depth.min(depth_frames.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|frame| frame.current_alias_column_list_source.clone())
}

fn nearest_join_using_tables(depth_frames: &[ParserDepthFrame], depth: usize) -> Vec<String> {
    depth_frames[..=depth.min(depth_frames.len().saturating_sub(1))]
        .iter()
        .rev()
        .find(|frame| !frame.join_using_tables.is_empty())
        .map(|frame| frame.join_using_tables.clone())
        .unwrap_or_default()
}

fn nearest_excluded_target_table(
    depth_frames: &[ParserDepthFrame],
    depth: usize,
) -> Option<String> {
    depth_frames[..=depth.min(depth_frames.len().saturating_sub(1))]
        .iter()
        .rev()
        .find_map(|frame| {
            frame
                .postgres_conflict_update_active
                .then(|| frame.current_target_table.clone())
                .flatten()
        })
}

fn current_scope_relation_tables(
    parsed_tables: &[ParsedTableEntry],
    scope_stack: &[usize],
) -> Vec<String> {
    let current_scope_id = *scope_stack.last().unwrap_or(&0);
    let mut tables = Vec::new();
    let mut seen = HashSet::new();

    for entry in parsed_tables {
        if entry.scope_id != current_scope_id {
            continue;
        }

        let normalized = entry.table.name.to_ascii_uppercase();
        if seen.insert(normalized) {
            tables.push(entry.table.name.clone());
        }
    }

    tables
}

fn snapshot_cursor_state(
    depth: usize,
    query_depth: usize,
    parsed_tables: &[ParsedTableEntry],
    depth_frames: &[ParserDepthFrame],
    scope_stack: &[usize],
    visible_parent: &HashMap<usize, Option<usize>>,
    cte_visible_parent: &HashMap<usize, Option<usize>>,
) -> (
    SqlPhase,
    usize,
    Vec<usize>,
    Vec<usize>,
    Vec<String>,
    Option<String>,
) {
    let phase = depth_frames
        .get(depth)
        .map(|frame| frame.phase)
        .unwrap_or(SqlPhase::Initial);
    let focused_tables = match phase {
        SqlPhase::ConflictTargetList
        | SqlPhase::DmlSetTargetList
        | SqlPhase::InsertColumnList
        | SqlPhase::MergeInsertColumnList
        | SqlPhase::DmlReturningList => nearest_target_table(depth_frames, depth)
            .into_iter()
            .collect(),
        SqlPhase::JoinUsingColumnList => nearest_join_using_tables(depth_frames, depth),
        SqlPhase::CteColumnList | SqlPhase::RecursiveCteColumnList => {
            nearest_cte_name(depth_frames, depth).into_iter().collect()
        }
        SqlPhase::DerivedAliasColumnList => nearest_alias_column_list_source(depth_frames, depth)
            .into_iter()
            .collect(),
        SqlPhase::LockingColumnList => current_scope_relation_tables(parsed_tables, scope_stack),
        _ => Vec::new(),
    };
    let excluded_target_table = if matches!(
        phase,
        SqlPhase::DmlReturningList | SqlPhase::ReturningIntoTarget
    ) {
        None
    } else {
        nearest_excluded_target_table(depth_frames, depth)
    };
    (
        phase,
        query_depth,
        build_visible_scope_chain(scope_stack, visible_parent),
        build_visible_scope_chain(scope_stack, cte_visible_parent),
        focused_tables,
        excluded_target_table,
    )
}

#[derive(Debug, Clone)]
struct ParserDepthFrame {
    phase: SqlPhase,
    is_query_scope: bool,
    allows_leading_query_expression: bool,
    with_plsql_state: WithPlsqlState,
    statement_kind: StatementKind,
    open_cursor_active: bool,
    current_target_table: Option<String>,
    current_cte_name: Option<String>,
    current_alias_column_list_source: Option<String>,
    dml_set_active: bool,
    recent_relation_tables: Vec<String>,
    join_using_tables: Vec<String>,
    postgres_conflict_update_active: bool,
    paren_func: Option<String>,
    function_from_state: FunctionFromState,
    returning_clause_active: bool,
    locking_clause_active: bool,
    hierarchical_clause_active: bool,
    lock_table_list_active: bool,
    /// The statement at this depth is an object-privilege statement
    /// (`GRANT`/`REVOKE`/`AUDIT`/`NOAUDIT`). Its privilege/option list reuses
    /// DML keywords (`SELECT`, `INSERT`, `UPDATE`, `DELETE`, …) that would
    /// otherwise flip the phase into a query/DML context, and its `TO`/`FROM`
    /// grantee slot names a user, not a relation. While set, keyword phase
    /// dispatch is skipped so the cursor stays in a neutral phase; the lone
    /// object slot (`… ON <object>`) is supplied downstream by
    /// `expected_object_suggestion_kind`.
    privilege_stmt_active: bool,
}

fn reset_relation_lookbehind(
    relation_modifier_state: &mut RelationModifierState,
    expectation: &mut Expectation,
    last_word: &mut Option<String>,
) {
    relation_modifier_state.clear();
    expectation.clear();
    *last_word = None;
}

fn start_with_plsql_declaration(frame: &mut ParserDepthFrame, keyword: &str) {
    frame.with_plsql_state = WithPlsqlState::Collecting {
        active_body_frames: Vec::new(),
        pending_routine_declaration: Some(WithPlsqlPendingDeclaration {
            starts_body: sql_text::with_plsql_declaration_starts_routine_body(keyword),
        }),
        pending_end: false,
    };
}

fn pop_with_plsql_body_frame(
    active_body_frames: &mut Vec<WithPlsqlBodyFrame>,
    expected_kind: Option<WithPlsqlBodyFrameKind>,
) {
    if let Some(expected_kind) = expected_kind {
        if active_body_frames
            .last()
            .is_some_and(|frame| frame.kind == expected_kind)
        {
            let _ = active_body_frames.pop();
            return;
        }

        if let Some(frame_idx) = active_body_frames
            .iter()
            .rposition(|frame| frame.kind == expected_kind)
        {
            active_body_frames.remove(frame_idx);
            return;
        }
    }

    let _ = active_body_frames.pop();
}

fn track_with_plsql_collecting_word(frame: &mut ParserDepthFrame, keyword: &str) {
    let WithPlsqlState::Collecting {
        active_body_frames,
        pending_routine_declaration,
        pending_end,
    } = &mut frame.with_plsql_state
    else {
        return;
    };

    let mut consumed_end_qualifier = false;
    if *pending_end {
        match keyword {
            "CASE" => {
                pop_with_plsql_body_frame(active_body_frames, Some(WithPlsqlBodyFrameKind::Case));
                consumed_end_qualifier = true;
            }
            "IF" => {
                pop_with_plsql_body_frame(active_body_frames, Some(WithPlsqlBodyFrameKind::If));
                consumed_end_qualifier = true;
            }
            "LOOP" => {
                pop_with_plsql_body_frame(active_body_frames, Some(WithPlsqlBodyFrameKind::Loop));
                consumed_end_qualifier = true;
            }
            _ => {
                pop_with_plsql_body_frame(active_body_frames, None);
            }
        }
        *pending_end = false;
    }

    if consumed_end_qualifier {
        return;
    }

    if sql_text::is_with_plsql_declaration_keyword(keyword) {
        *pending_routine_declaration = Some(WithPlsqlPendingDeclaration {
            starts_body: sql_text::with_plsql_declaration_starts_routine_body(keyword),
        });
        return;
    }

    if matches!(keyword, "AS" | "IS")
        && pending_routine_declaration.is_some_and(|declaration| declaration.starts_body)
    {
        active_body_frames.push(WithPlsqlBodyFrame::routine());
        *pending_routine_declaration = None;
        return;
    }

    match keyword {
        "BEGIN" => {
            if let Some(body_frame) = active_body_frames.last_mut() {
                if body_frame.awaiting_begin {
                    body_frame.awaiting_begin = false;
                } else {
                    active_body_frames
                        .push(WithPlsqlBodyFrame::nested(WithPlsqlBodyFrameKind::Block));
                }
            }
        }
        "DECLARE" => {
            if !active_body_frames.is_empty() {
                // A nested anonymous block starts at DECLARE and is completed by
                // the matching END after its BEGIN. Track it as a single frame
                // that is still waiting for BEGIN instead of double-counting the
                // DECLARE and BEGIN tokens as separate blocks.
                active_body_frames.push(WithPlsqlBodyFrame::awaiting_begin(
                    WithPlsqlBodyFrameKind::Block,
                ));
            }
        }
        "CASE" => {
            if !active_body_frames.is_empty() {
                active_body_frames.push(WithPlsqlBodyFrame::nested(WithPlsqlBodyFrameKind::Case));
            }
        }
        "IF" => {
            if !active_body_frames.is_empty() {
                active_body_frames.push(WithPlsqlBodyFrame::nested(WithPlsqlBodyFrameKind::If));
            }
        }
        "LOOP" => {
            if !active_body_frames.is_empty() {
                active_body_frames.push(WithPlsqlBodyFrame::nested(WithPlsqlBodyFrameKind::Loop));
            }
        }
        "END" => {
            if !active_body_frames.is_empty() {
                *pending_end = true;
            }
        }
        _ => {}
    }
}

fn handle_with_plsql_separator(frame: &mut ParserDepthFrame) -> bool {
    match &mut frame.with_plsql_state {
        WithPlsqlState::None => false,
        WithPlsqlState::AwaitingMainQuery => true,
        WithPlsqlState::Collecting {
            active_body_frames,
            pending_routine_declaration,
            pending_end,
        } => {
            if *pending_end {
                pop_with_plsql_body_frame(active_body_frames, None);
                *pending_end = false;
            }
            *pending_routine_declaration = None;

            if active_body_frames.is_empty() {
                frame.with_plsql_state = WithPlsqlState::AwaitingMainQuery;
                true
            } else {
                false
            }
        }
    }
}

fn push_completed_cte(
    parsed_ctes: &mut Vec<ParsedCteEntry>,
    open_cte: OpenCteDefinition,
    body_end: usize,
) {
    parsed_ctes.push(ParsedCteEntry {
        scope_id: open_cte.header.scope_id,
        visible_from_token: open_cte.header.visible_from_token,
        body_scope_id: open_cte.body_scope_id,
        self_visible_from_token: None,
        cte: CteDefinition {
            name: open_cte.header.name,
            depth: open_cte.header.depth,
            explicit_columns: open_cte.header.explicit_columns,
            explicit_column_range: open_cte.header.explicit_column_range,
            body_range: TokenRange {
                start: open_cte.body_start,
                end: body_end.max(open_cte.body_start),
            },
        },
    });
}

fn snapshot_open_ctes(
    open_cte_stack: &[OpenCteDefinition],
    body_end: usize,
) -> Vec<ParsedCteEntry> {
    open_cte_stack
        .iter()
        .map(|open_cte| ParsedCteEntry {
            scope_id: open_cte.header.scope_id,
            visible_from_token: open_cte.header.visible_from_token,
            body_scope_id: open_cte.body_scope_id,
            self_visible_from_token: None,
            cte: CteDefinition {
                name: open_cte.header.name.clone(),
                depth: open_cte.header.depth,
                explicit_columns: open_cte.header.explicit_columns.clone(),
                explicit_column_range: open_cte.header.explicit_column_range,
                body_range: TokenRange {
                    start: open_cte.body_start,
                    end: body_end.max(open_cte.body_start),
                },
            },
        })
        .collect()
}

fn close_parenthesis_scope(
    parser_state: &mut SplitState,
    depth: &mut usize,
    query_depth: &mut usize,
    depth_frames: &mut Vec<ParserDepthFrame>,
    scope_stack: &mut Vec<usize>,
) {
    if depth_frames
        .get(*depth)
        .map(|frame| frame.is_query_scope)
        .unwrap_or(false)
        && *depth > 0
    {
        *query_depth = query_depth.saturating_sub(1);
    }

    parser_state.pop_close_paren(')');
    *depth = parser_state.paren_depth();

    if scope_stack.len() > 1 {
        scope_stack.pop();
    }
    if depth_frames.len() > 1 {
        depth_frames.pop();
    }
}

fn phase_on_open_paren(
    tokens: &[SqlToken],
    open_paren_idx: usize,
    current_phase: SqlPhase,
    statement_kind: StatementKind,
) -> Option<SqlPhase> {
    if matches!(current_phase, SqlPhase::JoinCondition)
        && previous_word_chain_matches(tokens, open_paren_idx, &["USING"])
    {
        return Some(SqlPhase::JoinUsingColumnList);
    }

    if matches!(statement_kind, StatementKind::Insert)
        && previous_word_chain_matches(tokens, open_paren_idx, &["CONFLICT", "ON"])
    {
        return Some(SqlPhase::ConflictTargetList);
    }

    if matches!(current_phase, SqlPhase::IntoClause)
        && matches!(statement_kind, StatementKind::Insert)
    {
        let (prev_token, _) = prev_non_comment_token(tokens, open_paren_idx)?;

        return match prev_token {
            SqlToken::Word(word) if is_identifier_word_token(word) => {
                Some(SqlPhase::InsertColumnList)
            }
            SqlToken::Symbol(sym) if sym == ")" => Some(SqlPhase::InsertColumnList),
            _ => None,
        };
    }

    if matches!(statement_kind, StatementKind::Merge)
        && matches!(current_phase, SqlPhase::SetClause)
    {
        return prev_word_upper(tokens, open_paren_idx).and_then(|(prev_word, _)| {
            if prev_word == "INSERT" {
                Some(SqlPhase::MergeInsertColumnList)
            } else {
                None
            }
        });
    }

    None
}

fn alias_column_list_source_before_open_paren(
    tokens: &[SqlToken],
    open_paren_idx: usize,
    current_phase: SqlPhase,
) -> Option<String> {
    if !matches!(current_phase, SqlPhase::FromClause) {
        return None;
    }

    let (alias_token, alias_idx) = prev_non_comment_token(tokens, open_paren_idx)?;
    let SqlToken::Word(alias_word) = alias_token else {
        return None;
    };
    if !is_identifier_word_token(alias_word) {
        return None;
    }

    let alias_upper = alias_word.to_ascii_uppercase();
    if !is_quoted_identifier(alias_word)
        && (is_relation_alias_breaker(&alias_upper)
            || is_relation_postfix_column_list_keyword(&alias_upper))
    {
        return None;
    }

    let (source_token, _) = prev_non_comment_token(tokens, alias_idx)?;
    match source_token {
        SqlToken::Symbol(sym) if sym == ")" => Some(strip_identifier_quotes(alias_word)),
        SqlToken::Word(word) if is_identifier_word_token(word) => {
            let upper = word.to_ascii_uppercase();
            if is_relation_alias_breaker(&upper) || is_relation_postfix_column_list_keyword(&upper)
            {
                None
            } else {
                Some(strip_identifier_quotes(alias_word))
            }
        }
        _ => None,
    }
}

fn is_relation_postfix_column_list_keyword(word: &str) -> bool {
    matches!(
        word,
        "PARTITION"
            | "SUBPARTITION"
            | "SAMPLE"
            | "SEED"
            | "TABLESAMPLE"
            | "WITH"
            | "USE"
            | "FORCE"
            | "IGNORE"
            | "INDEXED"
            | "MODEL"
            | "PIVOT"
            | "UNPIVOT"
            | "MATCH_RECOGNIZE"
    )
}

fn push_recent_relation_table(frame: &mut ParserDepthFrame, table_name: &str) {
    if frame
        .recent_relation_tables
        .last()
        .is_some_and(|recent| recent.eq_ignore_ascii_case(table_name))
    {
        return;
    }

    frame.recent_relation_tables.push(table_name.to_string());
    if frame.recent_relation_tables.len() > 2 {
        frame
            .recent_relation_tables
            .drain(0..frame.recent_relation_tables.len().saturating_sub(2));
    }
}

fn begin_set_operator_operand_scope(
    scope_stack: &mut [usize],
    next_scope_id: &mut usize,
    scope_parent: &mut HashMap<usize, Option<usize>>,
    visible_parent: &mut HashMap<usize, Option<usize>>,
    cte_visible_parent: &mut HashMap<usize, Option<usize>>,
) {
    let Some(current_scope) = scope_stack.last_mut() else {
        return;
    };

    let cte_parent_scope = *current_scope;
    let parent_scope = visible_parent.get(current_scope).copied().unwrap_or(None);
    let operand_scope = *next_scope_id;
    *next_scope_id = next_scope_id.saturating_add(1);
    scope_parent.insert(operand_scope, Some(cte_parent_scope));
    visible_parent.insert(operand_scope, parent_scope);
    cte_visible_parent.insert(operand_scope, Some(cte_parent_scope));
    *current_scope = operand_scope;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatementKind {
    Unknown,
    Insert,
    Update,
    Delete,
    Merge,
    CreateTable,
    Fetch,
    ExecuteImmediate,
    OpenCursor,
    Rename,
    Lock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionFromState {
    NotApplicable,
    Available,
    Consumed,
}

impl FunctionFromState {
    fn from_function_name(function_name: Option<&str>) -> Self {
        if function_name.is_some_and(is_from_consuming_function) {
            Self::Available
        } else {
            Self::NotApplicable
        }
    }

    fn consume(&mut self) {
        if matches!(self, Self::Available) {
            *self = Self::Consumed;
        }
    }

    fn should_treat_from_as_function_argument(self) -> bool {
        matches!(self, Self::Available)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelationModifierState {
    None,
    LateralLikePending,
}

impl RelationModifierState {
    fn mark_lateral_like(&mut self) {
        *self = Self::LateralLikePending;
    }

    fn clear(&mut self) {
        *self = Self::None;
    }

    fn blocks_outer_scope_cutoff(self) -> bool {
        matches!(self, Self::LateralLikePending)
    }
}

fn is_row_limiting_fetch_clause(tokens: &[SqlToken], fetch_idx: usize) -> bool {
    matches!(
        next_word_upper(tokens, fetch_idx + 1),
        Some((next_keyword, _)) if matches!(next_keyword.as_str(), "FIRST" | "NEXT")
    )
}

fn transition_on_fetch_keyword(
    tokens: &[SqlToken],
    idx: usize,
    current_phase: SqlPhase,
) -> Option<(SqlPhase, StatementKind, Expectation)> {
    if is_row_limiting_fetch_clause(tokens, idx) {
        return Some((
            SqlPhase::OrderByClause,
            StatementKind::Unknown,
            Expectation::None,
        ));
    }

    if current_phase.is_column_context() || matches!(current_phase, SqlPhase::ValuesClause) {
        return None;
    }

    Some((SqlPhase::Initial, StatementKind::Fetch, Expectation::None))
}

fn transition_on_into_keyword(
    tokens: &[SqlToken],
    idx: usize,
    current_phase: SqlPhase,
    current_statement_kind: StatementKind,
    in_returning_clause: bool,
) -> Option<(SqlPhase, Expectation)> {
    let is_log_errors_target = matches!(
        current_statement_kind,
        StatementKind::Insert
            | StatementKind::Update
            | StatementKind::Delete
            | StatementKind::Merge
    ) && is_log_errors_into_clause(tokens, idx);

    if is_log_errors_target {
        return Some((SqlPhase::IntoClause, Expectation::Table));
    }

    if in_returning_clause {
        return Some((SqlPhase::ReturningIntoTarget, Expectation::Variable));
    }

    if matches!(current_statement_kind, StatementKind::Fetch) {
        return Some((SqlPhase::FetchIntoTarget, Expectation::Variable));
    }

    if matches!(current_statement_kind, StatementKind::ExecuteImmediate) {
        return Some((SqlPhase::ExecuteIntoTarget, Expectation::Variable));
    }

    if matches!(current_phase, SqlPhase::IntoClause) {
        return Some((SqlPhase::IntoClause, Expectation::Table));
    }

    let should_expect_table_target = matches!(
        current_statement_kind,
        StatementKind::Insert | StatementKind::Delete
    ) && matches!(
        current_phase,
        SqlPhase::SelectList | SqlPhase::Initial | SqlPhase::ValuesClause
    );
    let should_expect_merge_target = matches!(current_phase, SqlPhase::MergeTarget);
    let should_expect_set_clause_target = matches!(current_phase, SqlPhase::SetClause)
        && matches!(
            current_statement_kind,
            StatementKind::Insert | StatementKind::Update | StatementKind::Delete
        );

    if should_expect_table_target || should_expect_merge_target || should_expect_set_clause_target {
        return Some((SqlPhase::IntoClause, Expectation::Table));
    }

    if matches!(current_phase, SqlPhase::SelectList) {
        return Some((SqlPhase::SelectIntoTarget, Expectation::Variable));
    }

    None
}

fn transition_on_using_keyword(
    current_phase: SqlPhase,
    current_statement_kind: StatementKind,
    open_cursor_active: bool,
) -> Option<(SqlPhase, Expectation)> {
    if matches!(current_statement_kind, StatementKind::ExecuteImmediate) {
        return Some((SqlPhase::UsingBindList, Expectation::BindValue));
    }

    if matches!(
        current_statement_kind,
        StatementKind::Merge | StatementKind::Delete
    ) {
        return Some((SqlPhase::FromClause, Expectation::Table));
    }

    if matches!(current_phase, SqlPhase::FromClause) {
        return Some((SqlPhase::JoinCondition, Expectation::None));
    }

    if open_cursor_active {
        return Some((SqlPhase::UsingBindList, Expectation::BindValue));
    }

    None
}

fn should_start_execute_immediate(
    tokens: &[SqlToken],
    idx: usize,
    current_phase: SqlPhase,
) -> bool {
    if current_phase.is_column_context() || matches!(current_phase, SqlPhase::ValuesClause) {
        return false;
    }

    matches!(
        next_word_upper(tokens, idx + 1),
        Some((next_keyword, _)) if next_keyword == "IMMEDIATE"
    )
}

impl Default for ParserDepthFrame {
    fn default() -> Self {
        Self {
            phase: SqlPhase::Initial,
            is_query_scope: false,
            allows_leading_query_expression: false,
            with_plsql_state: WithPlsqlState::None,
            statement_kind: StatementKind::Unknown,
            open_cursor_active: false,
            current_target_table: None,
            current_cte_name: None,
            current_alias_column_list_source: None,
            dml_set_active: false,
            recent_relation_tables: Vec::new(),
            join_using_tables: Vec::new(),
            postgres_conflict_update_active: false,
            paren_func: None,
            function_from_state: FunctionFromState::NotApplicable,
            returning_clause_active: false,
            locking_clause_active: false,
            hierarchical_clause_active: false,
            lock_table_list_active: false,
            privilege_stmt_active: false,
        }
    }
}

/// Single-pass cursor parser:
/// - Tracks SQL phase/query depth at cursor
/// - Collects relation/subquery entries with scope ids
/// - Shares one keyword transition table for both phase and table collection
fn scan_cursor_context(tokens: &[SqlToken], cursor_token_len: usize) -> CursorScanResult {
    let mut parser_state = SplitState::default();
    let mut depth: usize = parser_state.paren_depth();
    let mut query_depth: usize = 0;
    let mut depth_frames: Vec<ParserDepthFrame> = vec![ParserDepthFrame::default()];
    let mut last_word: Option<String> = None;
    let mut relation_state = Expectation::None;
    let mut all_tables: Vec<ParsedTableEntry> = Vec::new();
    let mut all_subqueries: Vec<ParsedSubqueryEntry> = Vec::new();
    let mut all_ctes: Vec<ParsedCteEntry> = Vec::new();
    let mut subquery_tracks: Vec<(usize, usize)> = Vec::new(); // (depth, start_idx)

    let mut next_scope_id = 1usize;
    let mut scope_stack = vec![0usize];
    let mut scope_parent: HashMap<usize, Option<usize>> = HashMap::new();
    scope_parent.insert(0, None);
    let mut visible_parent: HashMap<usize, Option<usize>> = HashMap::new();
    visible_parent.insert(0, None);
    let mut cte_visible_parent: HashMap<usize, Option<usize>> = HashMap::new();
    cte_visible_parent.insert(0, None);

    let mut relation_modifier_state = RelationModifierState::None;
    let mut cte_state = CteState::Inactive;
    let mut pending_cte_header: Option<PendingCteHeader> = None;
    let mut open_cte_stack: Vec<OpenCteDefinition> = Vec::new();
    let mut cursor_open_ctes: Vec<ParsedCteEntry> = Vec::new();

    let mut cursor_snapshot: Option<(
        SqlPhase,
        usize,
        Vec<usize>,
        Vec<usize>,
        Vec<String>,
        Option<String>,
    )> = None;
    let mut idx = 0;

    let mark_query_scope =
        |depth: usize, depth_frames: &mut Vec<ParserDepthFrame>, query_depth: &mut usize| {
            if depth > 0
                && depth_frames
                    .get(depth)
                    .is_some_and(|frame| !frame.is_query_scope)
            {
                if let Some(frame) = depth_frames.get_mut(depth) {
                    frame.is_query_scope = true;
                }
                *query_depth = query_depth.saturating_add(1);
            }
        };

    while idx < tokens.len() {
        if cursor_snapshot.is_none() && idx == cursor_token_len {
            cursor_snapshot = Some(snapshot_cursor_state(
                depth,
                query_depth,
                &all_tables,
                &depth_frames,
                &scope_stack,
                &visible_parent,
                &cte_visible_parent,
            ));
            cursor_open_ctes = snapshot_open_ctes(&open_cte_stack, idx);
        }

        let token = &tokens[idx];

        match token {
            SqlToken::Symbol(sym) if sym == "(" => {
                let parent_phase = depth_frames
                    .get(depth)
                    .map(|frame| frame.phase)
                    .unwrap_or(SqlPhase::Initial);
                let parent_statement_kind = depth_frames
                    .get(depth)
                    .map(|frame| frame.statement_kind)
                    .unwrap_or(StatementKind::Unknown);
                let parent_target_table = depth_frames
                    .get(depth)
                    .and_then(|frame| frame.current_target_table.clone());
                let parent_cte_name = depth_frames
                    .get(depth)
                    .and_then(|frame| frame.current_cte_name.clone());
                let parent_alias_column_list_source = depth_frames
                    .get(depth)
                    .and_then(|frame| frame.current_alias_column_list_source.clone());
                let parent_recent_relation_tables = depth_frames
                    .get(depth)
                    .map(|frame| frame.recent_relation_tables.clone())
                    .unwrap_or_default();
                let parent_join_using_tables = depth_frames
                    .get(depth)
                    .map(|frame| frame.join_using_tables.clone())
                    .unwrap_or_default();
                let parent_postgres_conflict_update_active = depth_frames
                    .get(depth)
                    .map(|frame| frame.postgres_conflict_update_active)
                    .unwrap_or(false);
                let parent_scope_id = *scope_stack.last().unwrap_or(&0);
                let entering_cte_column_list = matches!(cte_state, CteState::AfterName);
                let entering_cte_body = matches!(cte_state, CteState::ExpectBody);
                let entering_alias_column_list_source =
                    alias_column_list_source_before_open_paren(tokens, idx, parent_phase);
                parser_state.push_open_paren('(');
                depth = parser_state.paren_depth();

                let inherited_phase = if matches!(cte_state, CteState::AfterName) {
                    SqlPhase::CteColumnList
                } else if entering_alias_column_list_source.is_some() {
                    SqlPhase::DerivedAliasColumnList
                } else if let Some(target_column_list_phase) =
                    phase_on_open_paren(tokens, idx, parent_phase, parent_statement_kind)
                {
                    target_column_list_phase
                } else if parent_phase.is_column_context()
                    || matches!(
                        parent_phase,
                        SqlPhase::ValuesClause | SqlPhase::IntoClause | SqlPhase::PivotClause
                    )
                {
                    parent_phase
                } else {
                    SqlPhase::Initial
                };
                if depth_frames.len() <= depth {
                    depth_frames.push(ParserDepthFrame::default());
                }
                if let Some(frame) = depth_frames.get_mut(depth) {
                    frame.phase = inherited_phase;
                    frame.is_query_scope = false;
                    frame.allows_leading_query_expression =
                        is_query_expression_start(tokens, idx + 1);
                    frame.statement_kind = parent_statement_kind;
                    frame.open_cursor_active = false;
                    frame.current_target_table = parent_target_table;
                    frame.current_cte_name = parent_cte_name;
                    frame.current_alias_column_list_source =
                        entering_alias_column_list_source.or(parent_alias_column_list_source);
                    frame.dml_set_active = false;
                    frame.recent_relation_tables = parent_recent_relation_tables;
                    frame.join_using_tables = parent_join_using_tables;
                    frame.postgres_conflict_update_active = parent_postgres_conflict_update_active;
                    // Record the function name that preceded this '(' so we can
                    // distinguish function-internal FROM from SQL FROM clauses.
                    frame.paren_func = last_word.take().map(|w| w.to_ascii_uppercase());
                    frame.function_from_state =
                        FunctionFromState::from_function_name(frame.paren_func.as_deref());
                    frame.returning_clause_active = false;
                    frame.locking_clause_active = false;
                    frame.hierarchical_clause_active = false;
                }

                let scope_id = next_scope_id;
                next_scope_id += 1;
                scope_stack.push(scope_id);
                scope_parent.insert(scope_id, Some(parent_scope_id));

                if entering_cte_column_list {
                    if let Some(header) = pending_cte_header.as_mut() {
                        if let Some((expr_range, _)) = extract_parenthesized_range(tokens, idx) {
                            header.explicit_column_range = Some(expr_range);
                            header.explicit_columns =
                                extract_cte_explicit_columns(tokens, expr_range);
                        }
                    }
                }
                if entering_cte_body {
                    if let Some(header) = pending_cte_header.take() {
                        open_cte_stack.push(OpenCteDefinition {
                            header,
                            body_depth: depth,
                            body_start: idx.saturating_add(1),
                            body_scope_id: scope_id,
                        });
                    }
                }

                let is_from_lateral_function = depth_frames
                    .get(depth)
                    .and_then(|frame| frame.paren_func.as_deref())
                    .is_some_and(is_implicitly_lateral_table_function);
                let inherited_visible_parent = if entering_cte_body
                    || (matches!(parent_phase, SqlPhase::FromClause)
                        && !relation_modifier_state.blocks_outer_scope_cutoff()
                        && !is_from_lateral_function)
                {
                    None
                } else {
                    Some(parent_scope_id)
                };
                visible_parent.insert(scope_id, inherited_visible_parent);
                // Row-source scope cutoffs should not hide statement-level WITH bindings.
                cte_visible_parent.insert(scope_id, Some(parent_scope_id));

                relation_modifier_state.clear();
                relation_state.clear();

                if matches!(parent_phase, SqlPhase::FromClause)
                    && is_query_expression_start(tokens, idx + 1)
                {
                    subquery_tracks.push((depth, idx + 1));
                }

                cte_state = cte_state.enter_body(depth).enter_explicit_column_list();
                idx += 1;
                continue;
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                while open_cte_stack
                    .last()
                    .is_some_and(|open_cte| open_cte.body_depth > depth)
                {
                    if let Some(open_cte) = open_cte_stack.pop() {
                        push_completed_cte(&mut all_ctes, open_cte, idx);
                    }
                }
                if open_cte_stack
                    .last()
                    .is_some_and(|open_cte| open_cte.body_depth == depth)
                {
                    if let Some(open_cte) = open_cte_stack.pop() {
                        push_completed_cte(&mut all_ctes, open_cte, idx);
                    }
                }
                cte_state = cte_state.close_parenthesis(depth);

                while subquery_tracks.last().is_some_and(|track| track.0 > depth) {
                    // Recover from malformed SQL with stale tracks.
                    subquery_tracks.pop();
                }

                let was_subquery = subquery_tracks.last().map(|t| t.0) == Some(depth);
                if let Some((_, start_idx)) = was_subquery.then(|| subquery_tracks.pop()).flatten()
                {
                    if start_idx <= idx {
                        let parent_scope_id = if scope_stack.len() >= 2 {
                            scope_stack[scope_stack.len() - 2]
                        } else {
                            0
                        };
                        let body_range = TokenRange {
                            start: start_idx,
                            end: idx,
                        };
                        if let Some((
                            alias,
                            next_idx,
                            body_end,
                            explicit_columns,
                            explicit_column_range,
                        )) = parse_subquery_alias(tokens, idx + 1)
                        {
                            let relation_name_for_tracking = alias.clone();
                            all_subqueries.push(ParsedSubqueryEntry {
                                subquery: SubqueryDefinition {
                                    alias: alias.clone(),
                                    source_relation: None,
                                    explicit_columns,
                                    explicit_column_range,
                                    body_range: TokenRange {
                                        start: body_range.start,
                                        end: body_end.max(body_range.end),
                                    },
                                    depth: depth.saturating_sub(1),
                                },
                                scope_id: parent_scope_id,
                            });
                            all_tables.push(ParsedTableEntry {
                                table: ScopedTableRef {
                                    name: alias.clone(),
                                    alias: Some(alias),
                                    depth: depth.saturating_sub(1),
                                    is_cte: false,
                                },
                                scope_id: parent_scope_id,
                                token_start: start_idx,
                                origin: ParsedTableEntryOrigin::DerivedAlias,
                            });
                            idx = next_idx;
                            close_parenthesis_scope(
                                &mut parser_state,
                                &mut depth,
                                &mut query_depth,
                                &mut depth_frames,
                                &mut scope_stack,
                            );
                            reset_relation_lookbehind(
                                &mut relation_modifier_state,
                                &mut relation_state,
                                &mut last_word,
                            );
                            if depth_frames
                                .get(depth)
                                .is_some_and(|frame| matches!(frame.phase, SqlPhase::FromClause))
                            {
                                if let Some(frame) = depth_frames.get_mut(depth) {
                                    push_recent_relation_table(frame, &relation_name_for_tracking);
                                }
                            }
                            continue;
                        }

                        let generated_name = anonymous_subquery_name(start_idx, depth);
                        let generated_name_for_tracking = generated_name.clone();
                        all_subqueries.push(ParsedSubqueryEntry {
                            subquery: SubqueryDefinition {
                                alias: generated_name.clone(),
                                source_relation: None,
                                explicit_columns: Vec::new(),
                                explicit_column_range: None,
                                body_range,
                                depth: depth.saturating_sub(1),
                            },
                            scope_id: parent_scope_id,
                        });
                        all_tables.push(ParsedTableEntry {
                            table: ScopedTableRef {
                                name: generated_name,
                                alias: None,
                                depth: depth.saturating_sub(1),
                                is_cte: false,
                            },
                            scope_id: parent_scope_id,
                            token_start: start_idx,
                            origin: ParsedTableEntryOrigin::DerivedAlias,
                        });
                        if depth_frames
                            .get(depth.saturating_sub(1))
                            .is_some_and(|frame| matches!(frame.phase, SqlPhase::FromClause))
                        {
                            if let Some(frame) = depth_frames.get_mut(depth.saturating_sub(1)) {
                                push_recent_relation_table(frame, &generated_name_for_tracking);
                            }
                        }
                    }
                }

                close_parenthesis_scope(
                    &mut parser_state,
                    &mut depth,
                    &mut query_depth,
                    &mut depth_frames,
                    &mut scope_stack,
                );
                reset_relation_lookbehind(
                    &mut relation_modifier_state,
                    &mut relation_state,
                    &mut last_word,
                );
                idx += 1;
                continue;
            }
            SqlToken::Comment(_) => {
                // Keep parser lookbehind state across comments so syntactic pairs like
                // `EXTRACT /*...*/ (...)` and `LATERAL /*...*/ (...)` are treated the
                // same as comment-free statements.
                idx += 1;
                continue;
            }
            SqlToken::String(_) => {
                idx += 1;
                continue;
            }
            SqlToken::Symbol(sym) if sym == "," => {
                relation_modifier_state.clear();
                let current_phase = depth_frames
                    .get(depth)
                    .map(|frame| frame.phase)
                    .unwrap_or(SqlPhase::Initial);
                let current_statement_kind = depth_frames
                    .get(depth)
                    .map(|frame| frame.statement_kind)
                    .unwrap_or(StatementKind::Unknown);
                let dml_set_active = depth_frames
                    .get(depth)
                    .is_some_and(|frame| frame.dml_set_active);
                let lock_table_list_active = depth_frames
                    .get(depth)
                    .is_some_and(|frame| frame.lock_table_list_active);
                if matches!(
                    current_phase,
                    SqlPhase::FromClause
                        | SqlPhase::PivotClause
                        | SqlPhase::ModelClause
                        | SqlPhase::MatchRecognizeClause
                ) {
                    depth_frames[depth].phase = SqlPhase::FromClause;
                    relation_state.expect_table();
                } else if dml_set_active && matches!(current_phase, SqlPhase::SetClause) {
                    depth_frames[depth].phase = SqlPhase::DmlSetTargetList;
                } else if matches!(current_statement_kind, StatementKind::Lock)
                    && lock_table_list_active
                {
                    // MySQL/MariaDB `LOCK TABLES t READ, ...` continues with the
                    // next table target after each comma.
                    depth_frames[depth].phase = SqlPhase::IntoClause;
                    relation_state.expect_table();
                }
                if matches!(cte_state, CteState::Inactive)
                    && depth_frames
                        .get(depth)
                        .is_some_and(|frame| matches!(frame.phase, SqlPhase::WithClause))
                {
                    cte_state = CteState::ExpectName;
                }
                idx += 1;
                continue;
            }
            SqlToken::Symbol(sym) if sym == "=" => {
                if depth_frames
                    .get(depth)
                    .is_some_and(|frame| frame.dml_set_active)
                    && depth_frames
                        .get(depth)
                        .is_some_and(|frame| matches!(frame.phase, SqlPhase::DmlSetTargetList))
                {
                    depth_frames[depth].phase = SqlPhase::SetClause;
                }
                idx += 1;
                continue;
            }
            SqlToken::Symbol(sym) if sym == ";" => {
                if handle_with_plsql_separator(&mut depth_frames[depth]) {
                    depth_frames[depth].phase = SqlPhase::WithClause;
                    depth_frames[depth].current_cte_name = None;
                    relation_state.clear();
                    last_word = None;
                    cte_state = CteState::ExpectName;
                    pending_cte_header = None;
                    idx += 1;
                    continue;
                }

                if matches!(
                    depth_frames
                        .get(depth)
                        .map(|frame| frame.with_plsql_state.clone()),
                    Some(WithPlsqlState::Collecting { .. })
                ) {
                    depth_frames[depth].phase = SqlPhase::Initial;
                    depth_frames[depth].current_cte_name = None;
                    relation_state.clear();
                    last_word = None;
                    pending_cte_header = None;
                    idx += 1;
                    continue;
                }

                let has_following_statement = tokens[idx + 1..]
                    .iter()
                    .any(|t| !matches!(t, SqlToken::Comment(_)));
                if idx >= cursor_token_len || !has_following_statement {
                    break;
                }

                all_tables.clear();
                all_subqueries.clear();
                all_ctes.clear();
                subquery_tracks.clear();
                pending_cte_header = None;
                open_cte_stack.clear();

                query_depth = 0;
                depth_frames = vec![ParserDepthFrame::default()];
                last_word = None;
                relation_state.clear();
                cte_state = CteState::Inactive;
                parser_state.clear_paren_stack();
                depth = 0;

                next_scope_id = 1;
                scope_stack = vec![0usize];
                scope_parent.clear();
                scope_parent.insert(0, None);
                visible_parent.clear();
                visible_parent.insert(0, None);
                cte_visible_parent.clear();
                cte_visible_parent.insert(0, None);
                relation_modifier_state.clear();

                idx += 1;
                continue;
            }
            SqlToken::Word(word) => {
                let upper = word.to_ascii_uppercase();

                match depth_frames
                    .get(depth)
                    .map(|frame| frame.with_plsql_state.clone())
                    .unwrap_or(WithPlsqlState::None)
                {
                    WithPlsqlState::Collecting { .. } => {
                        if let Some(frame) = depth_frames.get_mut(depth) {
                            track_with_plsql_collecting_word(frame, upper.as_str());
                        }
                        idx += 1;
                        continue;
                    }
                    WithPlsqlState::AwaitingMainQuery => {
                        if sql_text::is_with_plsql_declaration_keyword(&upper) {
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                start_with_plsql_declaration(frame, upper.as_str());
                                frame.current_cte_name = None;
                            }
                            pending_cte_header = None;
                            cte_state = CteState::Inactive;
                            idx += 1;
                            continue;
                        }

                        if let Some(frame) = depth_frames.get_mut(depth) {
                            frame.with_plsql_state = WithPlsqlState::None;
                        }
                    }
                    WithPlsqlState::None => {}
                }

                // CTE state machine
                match cte_state {
                    CteState::ExpectName if upper != "RECURSIVE" => {
                        if sql_text::is_with_main_query_keyword(&upper) {
                            cte_state = CteState::Inactive;
                            pending_cte_header = None;
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.current_cte_name = None;
                            }
                            // Process the main-query keyword normally below.
                        } else if is_with_plsql_declaration_keyword(upper.as_str()) {
                            cte_state = CteState::Inactive;
                            pending_cte_header = None;
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                start_with_plsql_declaration(frame, upper.as_str());
                                frame.current_cte_name = None;
                            }
                            idx += 1;
                            continue;
                        } else {
                            cte_state = CteState::AfterName;
                            pending_cte_header = Some(PendingCteHeader {
                                name: word.clone(),
                                depth: query_depth,
                                explicit_columns: Vec::new(),
                                explicit_column_range: None,
                                scope_id: *scope_stack.last().unwrap_or(&0),
                                visible_from_token: idx.saturating_add(1),
                            });
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.current_cte_name = Some(word.clone());
                            }
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.with_plsql_state = WithPlsqlState::None;
                            }
                            idx += 1;
                            continue;
                        }
                    }
                    CteState::AfterName => {
                        if upper == "AS" {
                            cte_state = CteState::ExpectBody;
                        } else if sql_text::is_cte_recovery_keyword(&upper) {
                            cte_state = CteState::Inactive;
                            pending_cte_header = None;
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.current_cte_name = None;
                            }
                            continue;
                        }
                        idx += 1;
                        continue;
                    }
                    CteState::ExpectAs => {
                        if upper == "AS" {
                            cte_state = CteState::ExpectBody;
                        } else if sql_text::is_cte_recovery_keyword(&upper) {
                            cte_state = CteState::Inactive;
                            pending_cte_header = None;
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.current_cte_name = None;
                            }
                            continue;
                        }
                        idx += 1;
                        continue;
                    }
                    CteState::InBody { .. } => {
                        // Inside CTE body, process normally for phase tracking at this depth
                        // but don't break out of CTE state
                    }
                    CteState::Inactive => {}
                    _ => {
                        idx += 1;
                        continue;
                    }
                }

                // Ensure phase_stack has entry for current depth
                while depth_frames.len() <= depth {
                    depth_frames.push(ParserDepthFrame::default());
                }

                let current_phase = depth_frames[depth].phase;
                let current_statement_kind = depth_frames
                    .get(depth)
                    .map(|frame| frame.statement_kind)
                    .unwrap_or(StatementKind::Unknown);

                // An object-privilege statement head
                // (`GRANT`/`REVOKE`/`AUDIT`/`NOAUDIT`): its privilege/option
                // list and grantee slot must not be parsed as a query/DML, so
                // the rest of the keyword dispatch is skipped for the whole
                // statement. The cursor stays in a neutral (General) phase; the
                // object slot after `ON` is completed downstream from
                // `expected_object_suggestion_kind`. Gated on a true statement
                // head (top level, untouched frame) so an identifier named
                // `grant`/`audit`/… elsewhere is unaffected.
                if depth == 0
                    && matches!(current_phase, SqlPhase::Initial)
                    && matches!(current_statement_kind, StatementKind::Unknown)
                    && last_word.is_none()
                    && matches!(upper.as_str(), "GRANT" | "REVOKE" | "AUDIT" | "NOAUDIT")
                {
                    depth_frames[depth].privilege_stmt_active = true;
                }
                if depth_frames[depth].privilege_stmt_active {
                    last_word = Some(upper);
                    idx += 1;
                    continue;
                }

                if is_insert_target_modifier_keyword(upper.as_str())
                    && should_promote_insert_modifier_to_target_context(
                        current_statement_kind,
                        current_phase,
                        last_word.as_deref(),
                    )
                {
                    // MySQL/MariaDB allow optional insert-target modifiers before
                    // the target table (`INSERT LOW_PRIORITY IGNORE t ...`).
                    // Move into table-target context so the next identifier is
                    // parsed as relation target even when `INTO` is omitted.
                    depth_frames[depth].phase = SqlPhase::IntoClause;
                    relation_state.expect_table();
                    last_word = Some(upper);
                    idx += 1;
                    continue;
                }

                if should_keep_waiting_for_insert_target_after_modifier(
                    current_statement_kind,
                    current_phase,
                    relation_state,
                    upper.as_str(),
                ) {
                    // Keep waiting for the actual insert/replace target after
                    // pre-target modifiers (`REPLACE LOW_PRIORITY INTO ...`).
                    relation_state.expect_table();
                    last_word = Some(upper);
                    idx += 1;
                    continue;
                }

                if upper == "LATERAL" && matches!(current_phase, SqlPhase::FromClause) {
                    relation_modifier_state.mark_lateral_like();
                    idx += 1;
                    continue;
                }
                if !(relation_modifier_state.blocks_outer_scope_cutoff()
                    && relation_state.is_expect_table())
                {
                    relation_modifier_state.clear();
                }

                match upper.as_str() {
                    "INSERT" => {
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        let is_merge_action_keyword = is_merge_action_context(
                            current_statement_kind,
                            current_phase,
                            last_word.as_deref(),
                        );
                        if is_merge_action_keyword {
                            // `MERGE ... WHEN ... THEN INSERT (...) VALUES (...)` reuses
                            // INSERT as an action keyword (no target table). Keep it in
                            // expression/column context instead of table-target context.
                            depth_frames[depth].phase = SqlPhase::SetClause;
                            relation_state.clear();
                        } else if is_expression_context {
                            // Inside expressions, INSERT can be a valid identifier/token.
                            relation_state.clear();
                        } else {
                            depth_frames[depth].statement_kind = StatementKind::Insert;
                            depth_frames[depth].current_target_table = None;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                            relation_state.clear();
                        }
                    }
                    "REPLACE" => {
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].current_target_table = None;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        // `CREATE OR REPLACE VIEW/PROCEDURE/FUNCTION/TRIGGER ...` uses
                        // REPLACE as a DDL modifier, not as a DML statement.
                        let is_create_or_replace = matches!(last_word.as_deref(), Some("OR"));
                        if is_expression_context || is_create_or_replace {
                            // Inside expressions, REPLACE can be a scalar function name.
                            // After `CREATE OR`, REPLACE is a DDL modifier, not DML.
                            relation_state.clear();
                        } else {
                            // MySQL `REPLACE [INTO] table ...` behaves like INSERT for
                            // completion purposes: expect a target relation right after
                            // REPLACE, even when INTO is omitted.
                            depth_frames[depth].phase = SqlPhase::IntoClause;
                            depth_frames[depth].statement_kind = StatementKind::Insert;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                            relation_state.expect_table();
                        }
                    }
                    "LOCK" => {
                        depth_frames[depth].postgres_conflict_update_active = false;
                        depth_frames[depth].lock_table_list_active = false;
                        let is_post_query_lock_modifier =
                            is_post_query_lock_clause(tokens, idx, current_phase);
                        if is_post_query_lock_modifier {
                            // MySQL/MariaDB `SELECT ... LOCK IN SHARE MODE` is a trailing
                            // query modifier, not a standalone `LOCK TABLE` statement.
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                            depth_frames[depth].locking_clause_active = true;
                            relation_state.clear();
                        }
                        if !is_post_query_lock_modifier {
                            depth_frames[depth].locking_clause_active = false;
                            let is_expression_context = current_phase.is_column_context()
                                || matches!(current_phase, SqlPhase::ValuesClause);
                            if is_expression_context {
                                // Inside expressions, LOCK can be a valid identifier/token.
                                relation_state.clear();
                            } else {
                                depth_frames[depth].statement_kind = StatementKind::Lock;
                                depth_frames[depth].phase = SqlPhase::Initial;
                                depth_frames[depth].lock_table_list_active = false;
                                relation_state.clear();
                            }
                        }
                    }
                    "OPEN" => {
                        depth_frames[depth].postgres_conflict_update_active = false;
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        if is_expression_context {
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::Initial;
                            depth_frames[depth].statement_kind = StatementKind::OpenCursor;
                            depth_frames[depth].open_cursor_active = false;
                            depth_frames[depth].returning_clause_active = false;
                            relation_state.clear();
                        }
                    }
                    "EXECUTE" if should_start_execute_immediate(tokens, idx, current_phase) => {
                        depth_frames[depth].phase = SqlPhase::Initial;
                        depth_frames[depth].statement_kind = StatementKind::ExecuteImmediate;
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        relation_state.clear();
                    }
                    "WITH" if with_starts_non_cte_query_head(tokens, idx) => {
                        // Some dialects use query-head `WITH` clauses that are not
                        // subquery-factoring clauses, such as SQL Server
                        // `WITH XMLNAMESPACES (...) SELECT ...`. These still start a
                        // query scope for nested-depth tracking, but they must not
                        // switch completion into CTE column-list semantics.
                        depth_frames[depth].phase = SqlPhase::Initial;
                        depth_frames[depth].current_cte_name = None;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        pending_cte_header = None;
                        cte_state = CteState::Inactive;
                        relation_state.clear();
                    }
                    "WITH" if with_starts_non_plsql_option(tokens, idx) => {
                        // Query-tail `WITH ...` options (`WITH READ ONLY`,
                        // `WITH [CASCADED|LOCAL] CHECK OPTION`, `FETCH ... WITH TIES`,
                        // `WITH NO DATA`, `WITH GRANT OPTION`, etc.) are not
                        // subquery-factoring clauses. Treat them as a post-query
                        // boundary so completion does not keep stale table/column
                        // scope from the preceding SELECT/FETCH clause.
                        depth_frames[depth].phase = SqlPhase::Initial;
                        depth_frames[depth].current_cte_name = None;
                        depth_frames[depth].locking_clause_active = false;
                        depth_frames[depth].hierarchical_clause_active = false;
                        relation_state.clear();
                    }
                    "WITH"
                        if should_enter_with_clause(
                            tokens,
                            idx,
                            current_phase,
                            depth_frames
                                .get(depth)
                                .map(|frame| frame.statement_kind)
                                .unwrap_or(StatementKind::Unknown),
                            relation_state,
                            depth,
                            last_word.as_deref(),
                            depth_frames
                                .get(depth)
                                .map(|frame| frame.allows_leading_query_expression)
                                .unwrap_or(false),
                        ) =>
                    {
                        depth_frames[depth].phase = SqlPhase::WithClause;
                        depth_frames[depth].current_cte_name = None;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        pending_cte_header = None;
                        cte_state = CteState::ExpectName;
                        relation_state.clear();
                    }
                    "SELECT" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let preserve_insert_statement_kind = depth == 0
                            && matches!(current_statement_kind, StatementKind::Insert)
                            && depth_frames
                                .get(depth)
                                .and_then(|frame| frame.current_target_table.as_ref())
                                .is_some();
                        depth_frames[depth].phase = SqlPhase::SelectList;
                        if !preserve_insert_statement_kind {
                            depth_frames[depth].statement_kind = StatementKind::Unknown;
                        }
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        relation_state.clear();
                    }
                    "CALL" => {
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        if is_expression_context {
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::Initial;
                            depth_frames[depth].statement_kind = StatementKind::Unknown;
                            depth_frames[depth].current_target_table = None;
                            depth_frames[depth].dml_set_active = false;
                            depth_frames[depth].returning_clause_active = false;
                            depth_frames[depth].locking_clause_active = false;
                            depth_frames[depth].hierarchical_clause_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                            relation_state.clear();
                        }
                    }
                    "TABLE"
                        if !relation_state.is_expect_table()
                            && matches!(
                                current_phase,
                                SqlPhase::Initial | SqlPhase::WithClause
                            )
                            && previous_word_upper(tokens, idx).is_none() =>
                    {
                        // Standalone query-head TABLE expressions (including
                        // Oracle `WITH TYPE ...; TABLE (...)`) should reset the
                        // declaration/CTE phase before parsing the row source body.
                        depth_frames[depth].phase = SqlPhase::Initial;
                        depth_frames[depth].statement_kind = StatementKind::Unknown;
                        depth_frames[depth].current_target_table = None;
                        depth_frames[depth].dml_set_active = false;
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].locking_clause_active = false;
                        depth_frames[depth].hierarchical_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        relation_state.clear();
                    }
                    "FROM" => {
                        let from_belongs_to_distinct_predicate =
                            is_distinct_from_operator(tokens, idx)
                                && current_phase.is_column_context();
                        let should_treat_as_function_from = depth_frames
                            .get(depth)
                            .map(|frame| {
                                frame
                                    .function_from_state
                                    .should_treat_from_as_function_argument()
                            })
                            .unwrap_or(false);
                        if should_treat_as_function_from {
                            if let Some(frame) = depth_frames.get_mut(depth) {
                                frame.function_from_state.consume();
                            }
                        } else if from_belongs_to_distinct_predicate {
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::FromClause;
                            depth_frames[depth].recent_relation_tables.clear();
                            depth_frames[depth].join_using_tables.clear();
                            relation_state.expect_table();
                        }
                    }
                    "INTO" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let in_returning_clause = depth_frames
                            .get(depth)
                            .map(|frame| frame.returning_clause_active)
                            .unwrap_or(false);
                        if let Some((phase, expectation)) = transition_on_into_keyword(
                            tokens,
                            idx,
                            current_phase,
                            current_statement_kind,
                            in_returning_clause,
                        ) {
                            depth_frames[depth].phase = phase;
                            relation_state = expectation;
                        } else {
                            relation_state.clear();
                        }
                    }
                    "IN" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        if matches!(current_statement_kind, StatementKind::Lock)
                            && matches!(current_phase, SqlPhase::IntoClause)
                        {
                            // Oracle `LOCK TABLE ... IN <lock_mode> MODE` switches from
                            // table target to lock-mode keywords.
                            depth_frames[depth].phase = SqlPhase::Initial;
                            depth_frames[depth].lock_table_list_active = false;
                        }
                        relation_state.clear();
                    }
                    "READ" | "WRITE" | "LOCAL" | "LOW_PRIORITY" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        if matches!(current_statement_kind, StatementKind::Lock)
                            && depth_frames
                                .get(depth)
                                .is_some_and(|frame| frame.lock_table_list_active)
                            && !relation_state.is_expect_table()
                        {
                            // MySQL/MariaDB `LOCK TABLES` lock-mode modifiers are
                            // postfix controls, not relation targets.
                            depth_frames[depth].phase = SqlPhase::Initial;
                        }
                        relation_state.clear();
                    }
                    "OVERWRITE" if matches!(last_word.as_deref(), Some("INSERT")) => {
                        // Hive/Spark-style `INSERT OVERWRITE TABLE ...` keeps
                        // target relation context after OVERWRITE.
                        depth_frames[depth].phase = SqlPhase::IntoClause;
                        relation_state.expect_table();
                    }
                    "DIRECTORY" if matches!(last_word.as_deref(), Some("OVERWRITE")) => {
                        // `INSERT OVERWRITE DIRECTORY ...` targets a filesystem
                        // location rather than a table relation.
                        depth_frames[depth].phase = SqlPhase::Initial;
                        relation_state.clear();
                    }
                    "USING" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let open_cursor_active = depth_frames
                            .get(depth)
                            .map(|frame| frame.open_cursor_active)
                            .unwrap_or(false);
                        if let Some((phase, expectation)) = transition_on_using_keyword(
                            current_phase,
                            current_statement_kind,
                            open_cursor_active,
                        ) {
                            depth_frames[depth].phase = phase;
                            if matches!(phase, SqlPhase::JoinCondition)
                                && matches!(current_phase, SqlPhase::FromClause)
                            {
                                depth_frames[depth].join_using_tables =
                                    depth_frames[depth].recent_relation_tables.clone();
                            } else {
                                depth_frames[depth].join_using_tables.clear();
                            }
                            relation_state = expectation;
                        }
                    }
                    "TABLE" | "TABLES"
                        if ((upper == "TABLE"
                            && (has_table_target_statement_keyword_before_table(
                                tokens,
                                idx,
                                last_word.as_deref(),
                            ) || is_comment_on_target(tokens, idx, last_word.as_deref())
                                || is_create_table_target(tokens, idx)))
                            || (upper == "TABLES"
                                && has_lock_keyword_before_table(
                                    tokens,
                                    idx,
                                    last_word.as_deref(),
                                ))) =>
                    {
                        // DDL/DCL target object position (`TRUNCATE TABLE ...`,
                        // `LOCK TABLE ...`, `LOCK TABLES ...`, `ALTER TABLE ...`,
                        // `DROP TABLE ...`,
                        // `FLASHBACK TABLE ...`, `COMMENT ON TABLE ...`,
                        // `CREATE [GLOBAL TEMPORARY] TABLE ...`)
                        // should provide table-name completion.
                        if has_lock_keyword_before_table(tokens, idx, last_word.as_deref()) {
                            depth_frames[depth].statement_kind = StatementKind::Lock;
                            depth_frames[depth].lock_table_list_active = upper == "TABLES";
                        } else if is_create_table_target(tokens, idx) {
                            depth_frames[depth].statement_kind = StatementKind::CreateTable;
                            depth_frames[depth].current_target_table = None;
                            depth_frames[depth].lock_table_list_active = false;
                        } else {
                            depth_frames[depth].lock_table_list_active = false;
                        }
                        depth_frames[depth].phase = SqlPhase::IntoClause;
                        relation_state.expect_table();
                    }
                    "TABLE"
                        if matches!(current_phase, SqlPhase::IntoClause)
                            && relation_state.is_expect_table()
                            && !matches!(
                                tokens.get(skip_comment_tokens(tokens, idx + 1)),
                                Some(SqlToken::Symbol(sym)) if sym == "("
                            ) =>
                    {
                        // Optional `TABLE` introducer in DML target syntax
                        // (`INSERT INTO TABLE t`, `INSERT OVERWRITE TABLE t`).
                        relation_state.expect_table();
                    }

                    "VIEW"
                        if is_comment_on_target(tokens, idx, last_word.as_deref())
                            || is_comment_on_qualified_view_target(
                                tokens,
                                idx,
                                last_word.as_deref(),
                            ) =>
                    {
                        // `COMMENT ON VIEW ...`, `COMMENT ON MATERIALIZED VIEW ...`,
                        // and `COMMENT ON EDITIONING VIEW ...` use the same
                        // object-target position as COMMENT ON TABLE.
                        depth_frames[depth].phase = SqlPhase::IntoClause;
                        relation_state.expect_table();
                    }
                    "COLUMN" if is_comment_on_target(tokens, idx, last_word.as_deref()) => {
                        // `COMMENT ON COLUMN ...` starts from a table-qualified
                        // target (`table.column`) and should provide relation
                        // completions for the leading table/view segment.
                        depth_frames[depth].phase = SqlPhase::IntoClause;
                        relation_state.expect_table();
                    }
                    "REFERENCES" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        if matches!(current_statement_kind, StatementKind::CreateTable) {
                            // CREATE TABLE foreign-key clauses introduce a referenced
                            // relation target after `REFERENCES`.
                            depth_frames[depth].phase = SqlPhase::IntoClause;
                            relation_state.expect_table();
                        } else {
                            relation_state.clear();
                        }
                    }
                    "JOIN" | "APPLY" => {
                        if upper == "APPLY" {
                            relation_modifier_state.mark_lateral_like();
                        }
                        depth_frames[depth].phase = SqlPhase::FromClause;
                        depth_frames[depth].join_using_tables.clear();
                        relation_state.expect_table();
                    }
                    "STRAIGHT_JOIN" => {
                        if matches!(current_phase, SqlPhase::FromClause) {
                            depth_frames[depth].phase = SqlPhase::FromClause;
                            depth_frames[depth].join_using_tables.clear();
                            relation_state.expect_table();
                        }
                    }
                    "ON" => {
                        if matches!(current_phase, SqlPhase::FromClause) {
                            depth_frames[depth].phase = SqlPhase::JoinCondition;
                            depth_frames[depth].join_using_tables.clear();
                        } else if is_create_on_table_target(tokens, idx) {
                            depth_frames[depth].phase = SqlPhase::IntoClause;
                            relation_state.expect_table();
                        } else {
                            relation_state.clear();
                        }
                    }
                    "WHERE" => {
                        depth_frames[depth].phase = SqlPhase::WhereClause;
                        relation_state.clear();
                    }
                    "GROUP" => {
                        if !is_within_group_keyword(tokens, idx) {
                            if let Some((next_keyword, next_idx)) = next_word_upper(tokens, idx + 1)
                            {
                                if next_keyword == "BY" {
                                    depth_frames[depth].phase = SqlPhase::GroupByClause;
                                    idx = next_idx; // skip BY (and any interleaved comments)
                                }
                            }
                        }
                        relation_state.clear();
                    }
                    "HAVING" => {
                        depth_frames[depth].phase = SqlPhase::HavingClause;
                        relation_state.clear();
                    }
                    "ORDER" => {
                        if let Some(by_idx) = find_order_by_keyword(tokens, idx + 1) {
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                            idx = by_idx; // skip BY (and any interleaved comments)
                        }
                        relation_state.clear();
                    }
                    "FOR" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        if matches!(current_statement_kind, StatementKind::OpenCursor) {
                            depth_frames[depth].open_cursor_active = true;
                        } else if is_locking_for_clause(tokens, idx + 1) {
                            depth_frames[depth].locking_clause_active = true;
                            // Locking clauses (`FOR UPDATE [OF ...]`, `FOR SHARE [OF ...]`,
                            // `FOR NO KEY UPDATE [OF ...]`, `FOR KEY SHARE [OF ...]`)
                            // can accept column references after `OF`.
                            if locking_for_clause_has_of_target(tokens, idx + 1) {
                                depth_frames[depth].phase = SqlPhase::LockingColumnList;
                            } else {
                                depth_frames[depth].phase = SqlPhase::OrderByClause;
                            }
                        } else if is_read_consistency_for_clause(tokens, idx + 1) {
                            // Read-consistency qualifiers (`FOR READ ONLY`, `FOR READ WRITE`)
                            // are end-of-query boundaries and must not keep table context.
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                        } else if is_post_query_for_clause(tokens, idx + 1)
                            || matches!(current_phase, SqlPhase::FromClause)
                        {
                            // Dialect-specific trailing clauses such as SQL Server
                            // `FOR JSON` / `FOR XML` / `FOR BROWSE` appear after
                            // FROM/WHERE and are not
                            // relation-target contexts.
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                        }
                        relation_state.clear();
                    }
                    "WAIT" | "NOWAIT" => {
                        if depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.locking_clause_active)
                            && !locking_of_clause_identifier_position(tokens, idx)
                        {
                            // Oracle lock options after `FOR UPDATE [OF ...]` are trailing
                            // modifiers, not expression/table contexts. Keep identifier
                            // suggestions for `OF wait` / `OF nowait` column names.
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                            relation_state.clear();
                        }
                    }
                    "SKIP" => {
                        let locking_clause_active = depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.locking_clause_active)
                            && (!is_for_update_of_identifier_slot(tokens, idx)
                                || matches!(
                                    next_word_upper(tokens, idx + 1),
                                    Some((next, _)) if next == "LOCKED"
                                ));

                        if locking_clause_active {
                            // Oracle `FOR UPDATE ... SKIP LOCKED` and in-progress `... SKIP`
                            // lock options close lock target list just like NOWAIT/WAIT.
                            // Keep `SKIP` as an identifier candidate for `OF <column list>`
                            // when it appears in an identifier slot (`OF skip`, `, skip`,
                            // `t.skip`).
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                            relation_state.clear();
                        }
                    }
                    "LOCKED" => {
                        if depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.locking_clause_active)
                            && matches!(last_word.as_deref(), Some("SKIP"))
                        {
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                            relation_state.clear();
                        }
                    }
                    "WINDOW" => {
                        // Treat SQL-standard WINDOW clause expressions as column context.
                        depth_frames[depth].phase = SqlPhase::OrderByClause;
                        relation_state.clear();
                    }
                    "QUALIFY" => {
                        // QUALIFY filters rows using analytic expressions, similar to WHERE.
                        depth_frames[depth].phase = SqlPhase::WhereClause;
                        relation_state.clear();
                    }
                    "LIMIT" | "OFFSET" => {
                        // Pagination clauses are post-FROM boundaries; they must not keep
                        // relation-target parsing active even when ORDER BY is omitted.
                        depth_frames[depth].phase = SqlPhase::OrderByClause;
                        relation_state.clear();
                    }
                    "FETCH" => {
                        if let Some((phase, statement_kind, expectation)) =
                            transition_on_fetch_keyword(tokens, idx, current_phase)
                        {
                            depth_frames[depth].phase = phase;
                            depth_frames[depth].statement_kind = statement_kind;
                            relation_state = expectation;
                        } else {
                            relation_state.clear();
                        }
                    }
                    "REJECT" | "CASCADE" | "RESTRICT" | "PURGE" | "REUSE" | "STORAGE" => {
                        if matches!(current_phase, SqlPhase::IntoClause)
                            && !relation_state.is_expect_table()
                        {
                            // Post-target modifiers in DML/DDL clauses (e.g.
                            // `LOG ERRORS ... REJECT`, `DROP ... CASCADE`,
                            // `TRUNCATE ... REUSE STORAGE`) should not remain
                            // in table-target completion context.
                            depth_frames[depth].phase = SqlPhase::Initial;
                        }
                        relation_state.clear();
                    }
                    "SET" => {
                        let hierarchical_clause_active = depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.hierarchical_clause_active);
                        let postgres_conflict_update_active = depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.postgres_conflict_update_active);
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let merge_update_set_introducer =
                            matches!(current_statement_kind, StatementKind::Merge)
                                && matches!(last_word.as_deref(), Some("UPDATE"));
                        let keep_expression_phase_for_set =
                            is_statement_keyword_suppressed_in_expression_phase(current_phase)
                                && !postgres_conflict_update_active
                                && !matches!(current_statement_kind, StatementKind::Update)
                                && !merge_update_set_introducer;
                        if hierarchical_clause_active {
                            // Oracle hierarchical query SEARCH/CYCLE clauses use
                            // `... SET <ordering_or_cycle_col>` where SET introduces
                            // a generated output column name rather than a column
                            // expression or DML assignment target.
                            depth_frames[depth].phase = SqlPhase::HierarchicalGeneratedColumnName;
                            depth_frames[depth].dml_set_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                        } else if matches!(current_phase, SqlPhase::RecursiveCteColumnList)
                            && matches!(cte_state, CteState::Inactive)
                        {
                            // Recursive CTE SEARCH/CYCLE clauses use `... SET <generated_col>`
                            // where SET introduces a generated output column name, not a DML
                            // target list or expression context.
                            depth_frames[depth].phase = SqlPhase::RecursiveCteGeneratedColumnName;
                            depth_frames[depth].dml_set_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                        } else if keep_expression_phase_for_set {
                            // Function-local operation keywords such as JSON_TRANSFORM `SET`
                            // can appear inside expression contexts. Keep the surrounding
                            // expression phase unless the token is the actual MERGE
                            // `... UPDATE SET ...` introducer, or a MySQL/MariaDB-style
                            // `UPDATE t JOIN ... ON ... SET ...` multi-table update target.
                            relation_state.clear();
                        } else if matches!(
                            current_statement_kind,
                            StatementKind::Insert | StatementKind::Update | StatementKind::Merge
                        ) {
                            depth_frames[depth].phase = SqlPhase::DmlSetTargetList;
                            depth_frames[depth].dml_set_active = true;
                        } else {
                            depth_frames[depth].phase = SqlPhase::SetClause;
                            depth_frames[depth].dml_set_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                        }
                        relation_state.clear();
                    }
                    "BY" => {
                        let hierarchical_clause_active = depth_frames
                            .get(depth)
                            .is_some_and(|frame| frame.hierarchical_clause_active);
                        if hierarchical_clause_active {
                            depth_frames[depth].phase = SqlPhase::OrderByClause;
                        } else if matches!(current_phase, SqlPhase::WithClause)
                            && matches!(cte_state, CteState::Inactive)
                            && is_recursive_cte_search_by_keyword(tokens, idx)
                        {
                            depth_frames[depth].phase = SqlPhase::RecursiveCteColumnList;
                        }
                        relation_state.clear();
                    }
                    "SEARCH" | "CYCLE" => {
                        if matches!(current_phase, SqlPhase::WithClause) {
                            // Recursive CTE SEARCH/CYCLE clauses operate on the recursive CTE
                            // output instead of the full visible scope. SEARCH becomes column
                            // context at BY; CYCLE becomes column context immediately.
                            depth_frames[depth].phase = if upper == "CYCLE" {
                                SqlPhase::RecursiveCteColumnList
                            } else {
                                SqlPhase::WithClause
                            };
                        } else if matches!(current_phase, SqlPhase::ConnectByClause) {
                            // Oracle hierarchical query clauses (`... SEARCH ...`,
                            // `... CYCLE ...`) appear after CONNECT BY and should
                            // keep expression-column semantics.
                            depth_frames[depth].phase = SqlPhase::ConnectByClause;
                            depth_frames[depth].hierarchical_clause_active = true;
                        }
                        relation_state.clear();
                    }
                    "RETURNING" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let is_dml_returning_context = matches!(
                            current_statement_kind,
                            StatementKind::Insert
                                | StatementKind::Update
                                | StatementKind::Delete
                                | StatementKind::Merge
                        );

                        if is_dml_returning_context {
                            // DML RETURNING lists target columns/expressions.
                            depth_frames[depth].phase = SqlPhase::DmlReturningList;
                            depth_frames[depth].returning_clause_active = true;
                            depth_frames[depth].locking_clause_active = false;
                        }
                        relation_state.clear();
                    }
                    "UPDATE" => {
                        depth_frames[depth].returning_clause_active = false;
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let is_lock_mode_update_keyword =
                            matches!(current_statement_kind, StatementKind::Lock)
                                && matches!(current_phase, SqlPhase::Initial)
                                && matches!(
                                    last_word.as_deref(),
                                    Some("IN") | Some("ROW") | Some("SHARE")
                                );
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        let is_merge_action_keyword = is_merge_action_context(
                            current_statement_kind,
                            current_phase,
                            last_word.as_deref(),
                        );
                        let is_mysql_conflict_update =
                            is_mysql_on_duplicate_key_update(tokens, idx);
                        let is_postgres_conflict_update =
                            is_postgres_on_conflict_do_update(tokens, idx);
                        let is_locking_update_keyword = matches!(last_word.as_deref(), Some("FOR"));
                        if is_lock_mode_update_keyword {
                            // Oracle `LOCK TABLE ... IN [ROW] SHARE UPDATE MODE` uses
                            // UPDATE as a lock-mode keyword, not a new DML statement.
                            depth_frames[depth].postgres_conflict_update_active = false;
                            relation_state.clear();
                        } else if is_locking_update_keyword {
                            // `FOR UPDATE OF ...` lock clause inside SELECT statements.
                            if locking_for_clause_has_of_target(tokens, idx) {
                                depth_frames[depth].phase = SqlPhase::LockingColumnList;
                            } else {
                                depth_frames[depth].phase = SqlPhase::OrderByClause;
                            }
                            depth_frames[depth].postgres_conflict_update_active = false;
                            relation_state.clear();
                        } else if is_merge_action_keyword
                            || is_mysql_conflict_update
                            || is_postgres_conflict_update
                        {
                            depth_frames[depth].locking_clause_active = false;
                            // `... ON DUPLICATE KEY UPDATE ...` and
                            // `... ON CONFLICT ... DO UPDATE ...` use UPDATE as an action keyword.
                            if is_mysql_conflict_update {
                                depth_frames[depth].phase = SqlPhase::DmlSetTargetList;
                                depth_frames[depth].dml_set_active = true;
                                depth_frames[depth].postgres_conflict_update_active = false;
                            } else if is_postgres_conflict_update {
                                depth_frames[depth].phase = SqlPhase::SetClause;
                                depth_frames[depth].dml_set_active = false;
                                depth_frames[depth].postgres_conflict_update_active = true;
                            } else {
                                depth_frames[depth].phase = SqlPhase::SetClause;
                                depth_frames[depth].dml_set_active = false;
                                depth_frames[depth].postgres_conflict_update_active = false;
                            }
                            relation_state.clear();
                        } else if is_expression_context {
                            depth_frames[depth].locking_clause_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                            // Inside expressions, UPDATE can be a valid identifier/token.
                            relation_state.clear();
                        } else if matches!(last_word.as_deref(), Some("ON")) {
                            // `... REFERENCES t (...) ON UPDATE <action>` (and the MySQL
                            // column-default `... ON UPDATE CURRENT_TIMESTAMP`) use UPDATE
                            // as a referential-action keyword, not a DML statement. Only a
                            // fixed keyword follows, never a relation, so drop any
                            // table-target expectation for the slot.
                            depth_frames[depth].locking_clause_active = false;
                            depth_frames[depth].postgres_conflict_update_active = false;
                            depth_frames[depth].phase = SqlPhase::Initial;
                            relation_state.clear();
                        } else {
                            depth_frames[depth].locking_clause_active = false;
                            depth_frames[depth].phase = SqlPhase::UpdateTarget;
                            depth_frames[depth].statement_kind = StatementKind::Update;
                            depth_frames[depth].current_target_table = None;
                            depth_frames[depth].postgres_conflict_update_active = false;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                            relation_state.expect_table();
                        }
                    }
                    "DELETE" => {
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].locking_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        let is_merge_action_keyword = is_merge_action_context(
                            current_statement_kind,
                            current_phase,
                            last_word.as_deref(),
                        );
                        let is_merge_delete_where = is_merge_delete_where_action(
                            tokens,
                            idx,
                            current_statement_kind,
                            current_phase,
                        );
                        if is_expression_context {
                            // Inside expressions, DELETE can be a valid identifier/token.
                            relation_state.clear();
                        } else if matches!(last_word.as_deref(), Some("ON")) {
                            // `... REFERENCES t (...) ON DELETE <action>` is a foreign-key
                            // referential action, not a standalone DELETE statement: the
                            // slot accepts only `CASCADE`/`SET NULL`/… keywords, never a
                            // relation. Drop any table-target expectation so the slot is
                            // not classified as a table phase.
                            depth_frames[depth].phase = SqlPhase::Initial;
                            relation_state.clear();
                        } else if is_merge_action_keyword || is_merge_delete_where {
                            // `MERGE ... WHEN MATCHED THEN DELETE WHERE ...` DELETE is an
                            // action keyword, not a standalone DML target clause.
                            depth_frames[depth].phase = SqlPhase::WhereClause;
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::DeleteTarget;
                            depth_frames[depth].statement_kind = StatementKind::Delete;
                            depth_frames[depth].current_target_table = None;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                            relation_state.expect_table();
                        }
                    }
                    "MERGE" => {
                        depth_frames[depth].returning_clause_active = false;
                        depth_frames[depth].locking_clause_active = false;
                        depth_frames[depth].postgres_conflict_update_active = false;
                        depth_frames[depth].phase = SqlPhase::MergeTarget;
                        depth_frames[depth].statement_kind = StatementKind::Merge;
                        depth_frames[depth].current_target_table = None;
                        mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        relation_state.clear();
                    }
                    "RENAME" => {
                        depth_frames[depth].postgres_conflict_update_active = false;
                        let is_expression_context = current_phase.is_column_context()
                            || matches!(current_phase, SqlPhase::ValuesClause);
                        if is_expression_context {
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::IntoClause;
                            depth_frames[depth].statement_kind = StatementKind::Rename;
                            relation_state.expect_table();
                        }
                    }
                    "TO" => {
                        let current_statement_kind = depth_frames
                            .get(depth)
                            .map(|frame| frame.statement_kind)
                            .unwrap_or(StatementKind::Unknown);
                        if matches!(current_statement_kind, StatementKind::Rename)
                            && matches!(current_phase, SqlPhase::IntoClause)
                        {
                            depth_frames[depth].phase = SqlPhase::Initial;
                            depth_frames[depth].statement_kind = StatementKind::Unknown;
                        }
                        relation_state.clear();
                    }
                    "CONNECT" => {
                        if let Some((next_keyword, next_idx)) = next_word_upper(tokens, idx + 1) {
                            if next_keyword == "BY" {
                                depth_frames[depth].phase = SqlPhase::ConnectByClause;
                                idx = next_idx;
                            }
                        }
                        relation_state.clear();
                    }
                    "START" => {
                        if let Some((next_keyword, next_idx)) = next_word_upper(tokens, idx + 1) {
                            if next_keyword == "WITH" {
                                depth_frames[depth].phase = SqlPhase::StartWithClause;
                                idx = next_idx;
                            }
                        }
                        relation_state.clear();
                    }
                    "VALUES" => {
                        // In MySQL ON DUPLICATE KEY UPDATE, VALUES(col) is a function
                        // reference to the attempted-insert value, NOT the INSERT VALUES clause.
                        // Skip the phase change when we're inside a DML SET expression.
                        let in_set_expr = depth_frames.get(depth).is_some_and(|frame| {
                            frame.dml_set_active && matches!(frame.phase, SqlPhase::SetClause)
                        });
                        if !in_set_expr {
                            depth_frames[depth].phase = SqlPhase::ValuesClause;
                            mark_query_scope(depth, &mut depth_frames, &mut query_depth);
                        }
                        relation_state.clear();
                    }
                    "MATCH_RECOGNIZE" => {
                        depth_frames[depth].phase = SqlPhase::MatchRecognizeClause;
                        relation_state.clear();
                    }
                    "MATCH" => {
                        if let Some((next_keyword, next_idx)) = next_word_upper(tokens, idx + 1) {
                            if next_keyword == "RECOGNIZE" {
                                depth_frames[depth].phase = SqlPhase::MatchRecognizeClause;
                                relation_state.clear();
                                idx = next_idx;
                            }
                        }
                    }
                    "PIVOT" | "UNPIVOT" => {
                        depth_frames[depth].phase = SqlPhase::PivotClause;
                        relation_state.clear();
                    }
                    "MODEL" => {
                        depth_frames[depth].phase = SqlPhase::ModelClause;
                        relation_state.clear();
                    }
                    "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" => {
                        if is_multiset_set_operator(tokens, idx) {
                            relation_state.clear();
                        } else {
                            depth_frames[depth].phase = SqlPhase::Initial;
                            depth_frames[depth].hierarchical_clause_active = false;
                            relation_state.clear();
                            begin_set_operator_operand_scope(
                                &mut scope_stack,
                                &mut next_scope_id,
                                &mut scope_parent,
                                &mut visible_parent,
                                &mut cte_visible_parent,
                            );
                        }
                    }
                    kw if is_table_stop_keyword(kw) && relation_state.is_expect_table() => {
                        relation_state.clear();
                    }
                    _ => {
                        if relation_state.is_expect_table() {
                            if let Some((table_name, next_idx)) = parse_table_name_deep(tokens, idx)
                            {
                                let current_statement_kind = depth_frames
                                    .get(depth)
                                    .map(|frame| frame.statement_kind)
                                    .unwrap_or(StatementKind::Unknown);
                                let lock_table_list_active = depth_frames
                                    .get(depth)
                                    .is_some_and(|frame| frame.lock_table_list_active);
                                let create_table_target_consumed =
                                    matches!(current_phase, SqlPhase::IntoClause)
                                        && matches!(
                                            current_statement_kind,
                                            StatementKind::CreateTable
                                        );
                                let should_record_target_table = matches!(
                                    current_phase,
                                    SqlPhase::UpdateTarget
                                        | SqlPhase::DeleteTarget
                                        | SqlPhase::MergeTarget
                                ) || (matches!(
                                    current_phase,
                                    SqlPhase::IntoClause
                                ) && matches!(
                                    current_statement_kind,
                                    StatementKind::Insert | StatementKind::Merge
                                )
                                    && !is_log_errors_table_target(tokens, idx))
                                    || (matches!(current_phase, SqlPhase::FromClause)
                                        && matches!(current_statement_kind, StatementKind::Delete)
                                        && depth_frames
                                            .get(depth)
                                            .and_then(|f| f.current_target_table.as_ref())
                                            .is_none());
                                if should_record_target_table {
                                    depth_frames[depth].current_target_table =
                                        Some(table_name.clone());
                                }
                                let relation_name_hint = relation_function_name_hint(&table_name);
                                let has_immediate_argument_list = matches!(
                                    tokens.get(next_idx),
                                    Some(SqlToken::Symbol(sym)) if sym == "("
                                );
                                let can_consume_relation_arguments =
                                    matches!(current_phase, SqlPhase::FromClause)
                                        && has_immediate_argument_list
                                        && (relation_modifier_state.blocks_outer_scope_cutoff()
                                            || relation_name_hint.is_some());
                                let relation_arg_parsed = if can_consume_relation_arguments
                                    && has_immediate_argument_list
                                {
                                    extract_parenthesized_range(tokens, next_idx)
                                } else {
                                    None
                                };
                                let relation_arg_range =
                                    relation_arg_parsed.map(|(range, _)| range);
                                let relation_arg_end = relation_arg_parsed
                                    .map(|(_, arg_end_idx)| arg_end_idx)
                                    .unwrap_or(next_idx);
                                let relation_output_end = if relation_arg_range.is_some() {
                                    skip_relation_postfix_clauses(tokens, relation_arg_end)
                                } else {
                                    relation_arg_end
                                };
                                let relation_body_range = relation_arg_range.and_then(|_| {
                                    (idx < relation_output_end).then_some(TokenRange {
                                        start: idx,
                                        end: relation_output_end,
                                    })
                                });
                                let relation_arg_tokens = relation_body_range
                                    .map(|range| token_range_slice(tokens, range));
                                let (
                                    direct_alias,
                                    direct_after_alias,
                                    direct_explicit_columns,
                                    direct_explicit_column_range,
                                ) = parse_alias_deep_with_columns(tokens, relation_arg_end);
                                let (direct_alias, direct_after_alias) = sanitize_lock_table_alias(
                                    tokens,
                                    relation_arg_end,
                                    direct_alias,
                                    direct_after_alias,
                                    current_statement_kind,
                                    lock_table_list_active,
                                );
                                let derived_alias = parse_alias_after_derived_relation_clauses(
                                    tokens,
                                    relation_arg_end,
                                );
                                let (alias, after_alias, explicit_columns, explicit_column_range) =
                                    if let Some((
                                        alias,
                                        next_idx,
                                        _,
                                        explicit_columns,
                                        explicit_column_range,
                                    )) = derived_alias.as_ref()
                                    {
                                        (
                                            Some(alias.clone()),
                                            *next_idx,
                                            explicit_columns.clone(),
                                            *explicit_column_range,
                                        )
                                    } else {
                                        (
                                            direct_alias,
                                            direct_after_alias,
                                            direct_explicit_columns,
                                            direct_explicit_column_range,
                                        )
                                    };
                                let alias_present = alias.is_some();
                                let scope_id = *scope_stack.last().unwrap_or(&0);
                                let uses_virtual_alias_scope =
                                    relation_arg_tokens.is_some_and(|body_tokens| {
                                        relation_uses_virtual_alias_scope(&table_name, body_tokens)
                                    });
                                let virtual_relation_body_range = derived_alias
                                    .as_ref()
                                    .map(|(_, _, body_end, _, _)| TokenRange {
                                        start: idx,
                                        end: *body_end,
                                    })
                                    .or(relation_body_range)
                                    .or_else(|| {
                                        (!explicit_columns.is_empty()).then_some(TokenRange {
                                            start: idx,
                                            end: relation_output_end.max(idx.saturating_add(1)),
                                        })
                                    });
                                let table_scope_name = if uses_virtual_alias_scope {
                                    alias.clone().unwrap_or_else(|| table_name.clone())
                                } else {
                                    table_name.clone()
                                };
                                let relation_tracking_name = if !explicit_columns.is_empty() {
                                    alias.clone().unwrap_or_else(|| table_scope_name.clone())
                                } else {
                                    table_scope_name.clone()
                                };
                                if let (Some(alias_name), Some(body_range)) =
                                    (alias.as_ref(), virtual_relation_body_range)
                                {
                                    if uses_virtual_alias_scope
                                        || derived_alias.is_some()
                                        || !explicit_columns.is_empty()
                                    {
                                        all_subqueries.push(ParsedSubqueryEntry {
                                            subquery: SubqueryDefinition {
                                                alias: alias_name.clone(),
                                                source_relation: Some(table_scope_name.clone()),
                                                explicit_columns: explicit_columns.clone(),
                                                explicit_column_range,
                                                body_range,
                                                depth,
                                            },
                                            scope_id,
                                        });
                                    }
                                }
                                all_tables.push(ParsedTableEntry {
                                    table: ScopedTableRef {
                                        name: table_scope_name,
                                        alias,
                                        depth,
                                        is_cte: false,
                                    },
                                    scope_id,
                                    token_start: idx,
                                    origin: ParsedTableEntryOrigin::Relation,
                                });
                                if matches!(current_phase, SqlPhase::FromClause) {
                                    push_recent_relation_table(
                                        &mut depth_frames[depth],
                                        &relation_tracking_name,
                                    );
                                }
                                if let Some(SqlToken::Symbol(sym)) = tokens.get(after_alias) {
                                    if sym == "," {
                                        relation_modifier_state.clear();
                                        relation_state.expect_table();
                                        last_word = None;
                                        idx = after_alias + 1;
                                        continue;
                                    }
                                    if sym == "(" && !alias_present && !create_table_target_consumed
                                    {
                                        // Preserve table-function name for immediate
                                        // parenthesized argument scope handling.
                                        last_word = relation_name_hint;
                                        relation_modifier_state.mark_lateral_like();
                                    } else {
                                        last_word = None;
                                    }
                                } else {
                                    last_word = None;
                                }
                                if !matches!(tokens.get(after_alias), Some(SqlToken::Symbol(sym)) if sym == "(")
                                {
                                    relation_modifier_state.clear();
                                }
                                if create_table_target_consumed {
                                    // Once the CREATE TABLE target relation has been consumed,
                                    // subsequent tokens belong to the table-definition body or
                                    // table-option list rather than another table-name slot.
                                    depth_frames[depth].phase = SqlPhase::Initial;
                                }
                                relation_state.clear();
                                idx = after_alias;
                                continue;
                            }
                            relation_state.clear();
                        }
                    }
                }
                last_word = Some(upper);
            }
            _ => {
                relation_modifier_state.clear();
                last_word = None;
            }
        }
        idx += 1;
    }

    if cursor_snapshot.is_none() {
        cursor_snapshot = Some(snapshot_cursor_state(
            depth,
            query_depth,
            &all_tables,
            &depth_frames,
            &scope_stack,
            &visible_parent,
            &cte_visible_parent,
        ));
        cursor_open_ctes = snapshot_open_ctes(&open_cte_stack, cursor_token_len.min(tokens.len()));
    }
    while let Some(open_cte) = open_cte_stack.pop() {
        push_completed_cte(&mut all_ctes, open_cte, tokens.len());
    }
    update_cte_body_self_visibility(&mut all_ctes, &all_tables, &scope_parent, tokens);
    update_cte_body_self_visibility(&mut cursor_open_ctes, &all_tables, &scope_parent, tokens);
    let (
        phase,
        cursor_query_depth,
        cursor_visible_scope_chain,
        cursor_visible_cte_scope_chain,
        focused_tables,
        excluded_target_table,
    ) = cursor_snapshot.unwrap_or((
        SqlPhase::Initial,
        0usize,
        vec![0usize],
        vec![0usize],
        Vec::new(),
        None,
    ));

    CursorScanResult {
        phase,
        depth: cursor_query_depth,
        visible_scope_chain: cursor_visible_scope_chain,
        visible_cte_scope_chain: cursor_visible_cte_scope_chain,
        parsed_tables: all_tables,
        parsed_subqueries: all_subqueries,
        parsed_ctes: all_ctes,
        cursor_open_ctes,
        focused_tables,
        excluded_target_table,
    }
}

struct TableAnalysis {
    tables: Vec<ScopedTableRef>,
    subqueries: Vec<SubqueryDefinition>,
}

fn anonymous_subquery_name(start_idx: usize, depth: usize) -> String {
    format!("__SUBQUERY_{}_{}", depth, start_idx)
}

/// Collect all table references from the full statement, tracking depth.
/// Returns tables visible from the cursor's active scope chain.
fn collect_tables_deep(
    tokens: &[SqlToken],
    cursor_scope_chain: &[usize],
    cursor_token_len: usize,
) -> TableAnalysis {
    let scan_result = scan_cursor_context(tokens, cursor_token_len);
    filter_scope_entries(
        &scan_result.parsed_tables,
        &scan_result.parsed_subqueries,
        cursor_scope_chain,
    )
}

fn filter_scope_entries(
    parsed_tables: &[ParsedTableEntry],
    parsed_subqueries: &[ParsedSubqueryEntry],
    visible_scope_chain: &[usize],
) -> TableAnalysis {
    let visible_scope_ids: HashSet<usize> = visible_scope_chain.iter().copied().collect();

    let tables = parsed_tables
        .iter()
        .filter(|entry| visible_scope_ids.contains(&entry.scope_id))
        .map(|entry| entry.table.clone())
        .collect();

    let subqueries = parsed_subqueries
        .iter()
        .filter(|entry| visible_scope_ids.contains(&entry.scope_id))
        .map(|entry| entry.subquery.clone())
        .collect();

    TableAnalysis { tables, subqueries }
}

fn apply_cte_and_excluded_target_tables(
    tables_in_scope: &mut Vec<ScopedTableRef>,
    ctes: &[CteDefinition],
    depth: usize,
    excluded_target_table: Option<&str>,
) {
    for cte in ctes {
        // A CTE already merged in by an earlier iteration (same name, redefined
        // at a different scope depth) is updated in place to the applicable depth.
        if let Some(existing_idx) = tables_in_scope
            .iter()
            .position(|t| t.is_cte && t.name.eq_ignore_ascii_case(&cte.name))
        {
            if tables_in_scope[existing_idx].depth <= cte.depth {
                tables_in_scope[existing_idx] = ScopedTableRef {
                    name: cte.name.clone(),
                    alias: None,
                    depth: cte.depth,
                    is_cte: true,
                };
            }
            continue;
        }

        // The FROM/JOIN scope collector records every relation reference as a
        // physical table (`is_cte = false`); it does not cross-check the WITH
        // list. So a reference to this CTE is already present, mis-flagged. Flag
        // those references as CTEs in place — preserving their alias and depth —
        // instead of appending a duplicate bare scope entry. Multiple references
        // (e.g. `FROM c c1, c c2`) are all corrected.
        let mut flagged_reference = false;
        for table_ref in tables_in_scope.iter_mut() {
            if !table_ref.is_cte && table_ref.name.eq_ignore_ascii_case(&cte.name) {
                table_ref.is_cte = true;
                flagged_reference = true;
            }
        }
        if flagged_reference {
            continue;
        }

        // The CTE is in scope but not referenced in this query's FROM; expose it
        // as a referable relation under its bare name.
        tables_in_scope.push(ScopedTableRef {
            name: cte.name.clone(),
            alias: None,
            depth: cte.depth,
            is_cte: true,
        });
    }

    if let Some(excluded_target_table) = excluded_target_table {
        let already = tables_in_scope.iter().any(|table| {
            table.name.eq_ignore_ascii_case(excluded_target_table)
                && table
                    .alias
                    .as_deref()
                    .is_some_and(|alias| alias.eq_ignore_ascii_case("EXCLUDED"))
        });
        if !already {
            tables_in_scope.push(ScopedTableRef {
                name: excluded_target_table.to_string(),
                alias: Some("EXCLUDED".to_string()),
                depth,
                is_cte: false,
            });
        }
    }
}

fn apply_cte_and_excluded_target_tables_with_pos(
    tables_in_scope: &mut Vec<TableRefDeclaredAtCursor>,
    ctes: &[CteDefinition],
    depth: usize,
    excluded_target_table: Option<&str>,
    cursor_token_len: usize,
) {
    for cte in ctes {
        // A CTE already merged in by an earlier iteration (same name, redefined
        // at a different scope depth) is updated in place to the applicable
        // depth.
        if let Some(existing_idx) = tables_in_scope
            .iter()
            .position(|t| t.table.is_cte && t.table.name.eq_ignore_ascii_case(&cte.name))
        {
            if tables_in_scope[existing_idx].table.depth <= cte.depth {
                let existing_pos = tables_in_scope[existing_idx].token_start;
                tables_in_scope[existing_idx] = TableRefDeclaredAtCursor {
                    table: ScopedTableRef {
                        name: cte.name.clone(),
                        alias: None,
                        depth: cte.depth,
                        is_cte: true,
                    },
                    token_start: existing_pos,
                };
            }
            continue;
        }

        // The FROM/JOIN scope collector records every relation reference as a
        // physical table (`is_cte = false`); it does not cross-check the WITH
        // list. So a reference to this CTE is already present, mis-flagged. Flag
        // those references as CTEs in place — preserving their alias and depth —
        // instead of appending a duplicate bare scope entry.
        let mut flagged_reference = false;
        for table_ref in tables_in_scope.iter_mut() {
            if !table_ref.table.is_cte && table_ref.table.name.eq_ignore_ascii_case(&cte.name) {
                table_ref.table.is_cte = true;
                flagged_reference = true;
            }
        }
        if flagged_reference {
            continue;
        }

        // The CTE is in scope but not referenced in this query's FROM; expose
        // it as a referable relation under its bare name.
        tables_in_scope.push(TableRefDeclaredAtCursor {
            table: ScopedTableRef {
                name: cte.name.clone(),
                alias: None,
                depth: cte.depth,
                is_cte: true,
            },
            token_start: cursor_token_len,
        });
    }

    if let Some(excluded_target_table) = excluded_target_table {
        let already = tables_in_scope.iter().any(|table| {
            table.table.name.eq_ignore_ascii_case(excluded_target_table)
                && table
                    .table
                    .alias
                    .as_deref()
                    .is_some_and(|alias| alias.eq_ignore_ascii_case("EXCLUDED"))
        });
        if !already {
            tables_in_scope.push(TableRefDeclaredAtCursor {
                table: ScopedTableRef {
                    name: excluded_target_table.to_string(),
                    alias: Some("EXCLUDED".to_string()),
                    depth,
                    is_cte: false,
                },
                token_start: cursor_token_len,
            });
        }
    }
}

fn is_cursor_inside_cte_body(entry: &ParsedCteEntry, cursor_token_len: usize) -> bool {
    cursor_token_len >= entry.cte.body_range.start && cursor_token_len < entry.cte.body_range.end
}

fn is_cursor_inside_cte_self_visible_region(
    entry: &ParsedCteEntry,
    cursor_token_len: usize,
) -> bool {
    entry
        .self_visible_from_token
        .is_some_and(|visible_from| cursor_token_len >= visible_from)
}

fn should_prefer_cte_entry(candidate: &ParsedCteEntry, existing: &ParsedCteEntry) -> bool {
    candidate.self_visible_from_token.is_some() && existing.self_visible_from_token.is_none()
        || candidate.cte.body_range.end > existing.cte.body_range.end
}

fn filter_visible_ctes(
    parsed_ctes: &[ParsedCteEntry],
    visible_scope_chain: &[usize],
    cursor_token_len: usize,
) -> Vec<CteDefinition> {
    let visible_scope_ids: HashSet<usize> = visible_scope_chain.iter().copied().collect();
    let mut visible_entries: Vec<ParsedCteEntry> = Vec::new();

    for entry in parsed_ctes.iter().filter(|entry| {
        visible_scope_ids.contains(&entry.scope_id)
            && entry.visible_from_token <= cursor_token_len
            && (!is_cursor_inside_cte_body(entry, cursor_token_len)
                || is_cursor_inside_cte_self_visible_region(entry, cursor_token_len))
    }) {
        if let Some(existing) = visible_entries.iter_mut().find(|visible_entry| {
            visible_entry.scope_id == entry.scope_id
                && visible_entry.body_scope_id == entry.body_scope_id
                && visible_entry.cte.name.eq_ignore_ascii_case(&entry.cte.name)
        }) {
            if should_prefer_cte_entry(entry, existing) {
                *existing = entry.clone();
            }
            continue;
        }
        visible_entries.push(entry.clone());
    }

    visible_entries.into_iter().map(|entry| entry.cte).collect()
}

fn token_range_contains_cursor(range: TokenRange, cursor_token_len: usize) -> bool {
    !range.is_empty() && cursor_token_len >= range.start && cursor_token_len <= range.end
}

fn should_prefer_active_query_range(candidate: TokenRange, existing: TokenRange) -> bool {
    let candidate_len = candidate.end.saturating_sub(candidate.start);
    let existing_len = existing.end.saturating_sub(existing.start);
    candidate_len < existing_len
        || (candidate_len == existing_len && candidate.start >= existing.start)
}

fn find_active_query_range(
    tokens: &[SqlToken],
    parsed_ctes: &[ParsedCteEntry],
    parsed_subqueries: &[ParsedSubqueryEntry],
    cursor_token_len: usize,
) -> (Option<TokenRange>, bool) {
    let mut active_range = None;
    let mut in_set_operation_query_branch = false;

    let mut consider = |range: TokenRange| {
        if !token_range_contains_cursor(range, cursor_token_len) {
            return;
        }

        if active_range.is_none_or(|existing| should_prefer_active_query_range(range, existing)) {
            active_range = Some(range);
        }
    };

    for entry in parsed_ctes {
        consider(entry.cte.body_range);
    }

    for entry in parsed_subqueries {
        consider(entry.subquery.body_range);
    }

    // A CTE body and a FROM-derived table are not the only nested query scopes:
    // a scalar subquery in the SELECT list (`SELECT (SELECT | FROM b) FROM a`),
    // an IN/EXISTS subquery in a predicate (`WHERE x IN (SELECT | FROM b)`), and
    // any nested combination of these also scope the cursor to their own inner
    // query. They are neither CTEs nor row sources, so the scope state machine
    // does not record them as candidate ranges. Scanning the parenthesis
    // structure directly captures every `(`-introduced query expression
    // uniformly — closed, or still open while being typed — so the innermost one
    // containing the cursor wins by the same smallest-range preference.
    let mut paren_stack: Vec<(usize, bool)> = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        match token {
            SqlToken::Symbol(sym) if sym == "(" => {
                let is_query = is_query_expression_start(tokens, idx + 1);
                paren_stack.push((idx + 1, is_query));
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                if let Some((inner_start, is_query)) = paren_stack.pop() {
                    if is_query {
                        consider(TokenRange {
                            start: inner_start,
                            end: idx,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    // Parens still open at end-of-input enclose the cursor when a subquery is
    // mid-typing (`… IN (SELECT | FROM b`); their body runs to the statement end.
    for (inner_start, is_query) in paren_stack {
        if is_query {
            consider(TokenRange {
                start: inner_start,
                end: tokens.len(),
            });
        }
    }

    // Within the chosen query scope (or the whole statement, when the cursor is
    // not nested in a subquery), a set operator (`UNION`/`INTERSECT`/`MINUS`/
    // `EXCEPT`) splits the scope into independent branches, each with its own
    // select list and row sources. Narrow to the branch the cursor sits in so
    // projection-scoped completion (e.g. the `t.*` wildcard, which re-scans this
    // range for row sources) sees only that branch — matching the per-branch
    // column scope the state machine already resolves.
    let base = active_range.unwrap_or(TokenRange {
        start: 0,
        end: tokens.len(),
    });
    let branch = set_operation_branch_range(tokens, base, cursor_token_len);
    if branch != base {
        active_range = Some(branch);
        in_set_operation_query_branch = true;
    }

    (active_range, in_set_operation_query_branch)
}

/// Narrow `base` to the set-operation branch (`UNION`/`INTERSECT`/`MINUS`/
/// `EXCEPT` separated) containing the cursor. Set operators at the scope's own
/// nesting level (relative paren depth 0) delimit the branches; operators inside
/// a deeper subquery/parenthesised expression are ignored. Returns `base`
/// unchanged when the scope has no set operator.
fn set_operation_branch_range(
    tokens: &[SqlToken],
    base: TokenRange,
    cursor_token_len: usize,
) -> TokenRange {
    let start = base.start.min(tokens.len());
    let end = base.end.min(tokens.len());
    let mut depth = 0usize;
    let mut branch_start = start;
    let mut branch_end = end;
    for idx in start..end {
        match &tokens[idx] {
            SqlToken::Symbol(sym) if sym == "(" => depth += 1,
            SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
            SqlToken::Word(word)
                if depth == 0
                    && matches!(
                        word.to_ascii_uppercase().as_str(),
                        "UNION" | "INTERSECT" | "MINUS" | "EXCEPT"
                    ) =>
            {
                if is_multiset_set_operator(tokens, idx) {
                    continue;
                }
                if idx < cursor_token_len {
                    branch_start = idx + 1;
                } else {
                    branch_end = idx;
                    break;
                }
            }
            _ => {}
        }
    }
    TokenRange {
        start: branch_start,
        end: branch_end,
    }
}

pub(crate) fn token_range_slice(tokens: &[SqlToken], range: TokenRange) -> &[SqlToken] {
    let start = range.start.min(tokens.len());
    let end = range.end.min(tokens.len());
    if start >= end {
        &tokens[0..0]
    } else {
        &tokens[start..end]
    }
}

fn extract_cte_explicit_columns(tokens: &[SqlToken], range: TokenRange) -> Vec<String> {
    let expr_tokens = token_range_slice(tokens, range);
    let expr_depths = paren_depths(expr_tokens);
    let mut explicit_columns = Vec::new();

    for (expr_idx, token) in expr_tokens.iter().enumerate() {
        if !is_top_level_depth(&expr_depths, expr_idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            explicit_columns.push(word.clone());
        }
    }

    explicit_columns
}

fn extract_parenthesized_range(
    tokens: &[SqlToken],
    open_idx: usize,
) -> Option<(TokenRange, usize)> {
    match tokens.get(open_idx) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => {}
        _ => return None,
    }

    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    while idx < tokens.len() {
        match &tokens[idx] {
            SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
            SqlToken::Symbol(sym) if sym == ")" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((
                        TokenRange {
                            start: open_idx + 1,
                            end: idx,
                        },
                        idx + 1,
                    ));
                }
            }
            _ => {}
        }
        idx += 1;
    }

    Some((
        TokenRange {
            start: open_idx.saturating_add(1).min(tokens.len()),
            end: tokens.len(),
        },
        tokens.len(),
    ))
}

fn extract_preceding_parenthesized_range(
    tokens: &[SqlToken],
    before_idx: usize,
) -> Option<TokenRange> {
    let mut close_idx = before_idx.min(tokens.len());
    while close_idx > 0 {
        close_idx -= 1;
        match tokens.get(close_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(SqlToken::Symbol(sym)) if sym == ")" => break,
            _ => return None,
        }
    }

    let mut depth = 1usize;
    let mut idx = close_idx;
    while idx > 0 {
        idx -= 1;
        match tokens.get(idx) {
            Some(SqlToken::Symbol(sym)) if sym == ")" => depth = depth.saturating_add(1),
            Some(SqlToken::Symbol(sym)) if sym == "(" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(TokenRange {
                        start: idx + 1,
                        end: close_idx,
                    });
                }
            }
            _ => {}
        }
    }

    None
}

/// Parse CTE definitions from WITH clause.
#[cfg(test)]
fn parse_ctes(tokens: &[SqlToken]) -> Vec<CteDefinition> {
    let mut ctes = Vec::new();
    let mut idx = 0;

    // Find top-level WITH keyword
    let mut paren_state = ParenDepthState::default();
    let mut found_with = false;
    while idx < tokens.len() {
        let token = &tokens[idx];
        match token {
            SqlToken::Word(w) if paren_state.depth() == 0 && w.eq_ignore_ascii_case("WITH") => {
                idx += 1;
                found_with = true;
                break;
            }
            // If we hit a top-level statement keyword before WITH, no CTEs.
            SqlToken::Word(w) if paren_state.depth() == 0 => {
                let u = w.to_ascii_uppercase();
                if sql_text::is_with_main_query_keyword(&u) {
                    return ctes;
                }
            }
            _ => {}
        }
        apply_paren_token(&mut paren_state, token);
        idx += 1;
    }

    if !found_with {
        return ctes;
    }

    idx = skip_comment_tokens(tokens, idx);

    // Skip RECURSIVE if present
    if let Some(SqlToken::Word(w)) = tokens.get(idx) {
        if w.eq_ignore_ascii_case("RECURSIVE") {
            idx += 1;
        }
    }

    // Parse CTE definitions
    loop {
        if idx >= tokens.len() {
            break;
        }

        idx = skip_comment_tokens(tokens, idx);

        // Expect CTE name
        let cte_name = match tokens.get(idx) {
            Some(SqlToken::Word(w)) => {
                let u = w.to_ascii_uppercase();
                if matches!(
                    u.as_str(),
                    "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "MERGE"
                ) {
                    break;
                }
                if is_with_plsql_declaration_keyword(u.as_str()) {
                    break;
                }
                w.clone()
            }
            _ => break,
        };
        idx += 1;
        idx = skip_comment_tokens(tokens, idx);

        let mut explicit_columns = Vec::new();
        let mut explicit_column_range = None;

        // Check for explicit column list: cte_name(col1, col2)
        if let Some(SqlToken::Symbol(s)) = tokens.get(idx) {
            if s == "(" {
                if let Some((expr_range, next_idx)) = extract_parenthesized_range(tokens, idx) {
                    explicit_column_range = Some(expr_range);
                    explicit_columns = extract_cte_explicit_columns(tokens, expr_range);
                    idx = next_idx;
                }
            }
        }

        idx = skip_comment_tokens(tokens, idx);

        // Expect AS
        if let Some(SqlToken::Word(w)) = tokens.get(idx) {
            if w.eq_ignore_ascii_case("AS") {
                idx += 1;
            }
        }

        idx = skip_comment_tokens(tokens, idx);
        if let Some(SqlToken::Word(w)) = tokens.get(idx) {
            if w.eq_ignore_ascii_case("NOT") {
                let materialized_idx = skip_comment_tokens(tokens, idx + 1);
                if matches!(tokens.get(materialized_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("MATERIALIZED"))
                {
                    idx = materialized_idx + 1;
                }
            } else if w.eq_ignore_ascii_case("MATERIALIZED") {
                idx += 1;
            }
        }

        idx = skip_comment_tokens(tokens, idx);

        // Capture CTE body token range (balanced parens).
        let mut body_range = TokenRange::empty();
        if let Some(SqlToken::Symbol(s)) = tokens.get(idx) {
            if s == "(" {
                if let Some((captured_range, next_idx)) = extract_parenthesized_range(tokens, idx) {
                    idx = next_idx;
                    body_range = captured_range;
                }
            }
        }

        ctes.push(CteDefinition {
            name: cte_name,
            depth: 0,
            explicit_columns,
            explicit_column_range,
            body_range,
        });

        // Check for comma (another CTE) or end
        idx = skip_comment_tokens(tokens, idx);
        match tokens.get(idx) {
            Some(SqlToken::Symbol(s)) if s == "," => {
                idx += 1;
                continue;
            }
            _ => break,
        }
    }

    ctes
}

/// Peek at the next word token (skipping comments) and return its uppercase form.
fn next_word_upper(tokens: &[SqlToken], idx: usize) -> Option<(String, usize)> {
    let mut current_idx = idx;
    while current_idx < tokens.len() {
        match &tokens[current_idx] {
            SqlToken::Comment(_) => {
                current_idx += 1;
                continue;
            }
            SqlToken::Word(word) => {
                return Some((word.to_ascii_uppercase(), current_idx));
            }
            _ => return None,
        }
    }
    None
}

fn locking_of_clause_identifier_position(tokens: &[SqlToken], idx: usize) -> bool {
    let Some((prev_token, _)) = prev_non_comment_token(tokens, idx) else {
        return false;
    };

    match prev_token {
        SqlToken::Word(word) => word.eq_ignore_ascii_case("OF"),
        SqlToken::Symbol(symbol) => matches!(symbol.as_str(), "," | "."),
        _ => false,
    }
}

fn prev_word_upper(tokens: &[SqlToken], before_idx: usize) -> Option<(String, usize)> {
    let mut current_idx = before_idx;
    while current_idx > 0 {
        current_idx -= 1;
        match &tokens[current_idx] {
            SqlToken::Comment(_) => continue,
            SqlToken::Word(word) => return Some((word.to_ascii_uppercase(), current_idx)),
            _ => continue,
        }
    }
    None
}

fn prev_non_comment_token(tokens: &[SqlToken], before_idx: usize) -> Option<(&SqlToken, usize)> {
    let mut current_idx = before_idx;
    while current_idx > 0 {
        current_idx -= 1;
        match tokens.get(current_idx) {
            Some(SqlToken::Comment(_)) => continue,
            Some(token) => return Some((token, current_idx)),
            None => return None,
        }
    }
    None
}

fn is_for_update_of_identifier_slot(tokens: &[SqlToken], idx: usize) -> bool {
    let Some((prev_token, _)) = prev_non_comment_token(tokens, idx) else {
        return false;
    };

    match prev_token {
        SqlToken::Word(word) => word.eq_ignore_ascii_case("OF"),
        SqlToken::Symbol(sym) => matches!(sym.as_str(), "," | "."),
        _ => false,
    }
}

fn is_distinct_from_operator(tokens: &[SqlToken], from_idx: usize) -> bool {
    let Some((prev_word, prev_idx)) = prev_word_upper(tokens, from_idx) else {
        return false;
    };
    if prev_word != "DISTINCT" {
        return false;
    }

    let Some((second_prev_word, _)) = prev_word_upper(tokens, prev_idx) else {
        return false;
    };

    second_prev_word == "IS" || second_prev_word == "NOT"
}

fn is_within_group_keyword(tokens: &[SqlToken], group_idx: usize) -> bool {
    let Some((prev_word, _)) = prev_word_upper(tokens, group_idx) else {
        return false;
    };

    prev_word == "WITHIN"
}

fn previous_word_chain_matches(tokens: &[SqlToken], before_idx: usize, chain: &[&str]) -> bool {
    let mut cursor = before_idx;
    for expected in chain {
        let Some((word, word_idx)) = prev_word_upper(tokens, cursor) else {
            return false;
        };
        if word != *expected {
            return false;
        }
        cursor = word_idx;
    }
    true
}

fn is_mysql_on_duplicate_key_update(tokens: &[SqlToken], update_idx: usize) -> bool {
    previous_word_chain_matches(tokens, update_idx, &["KEY", "DUPLICATE", "ON"])
}

fn is_postgres_on_conflict_do_update(tokens: &[SqlToken], update_idx: usize) -> bool {
    let Some((prev_word, do_idx)) = prev_word_upper(tokens, update_idx) else {
        return false;
    };
    if prev_word != "DO" {
        return false;
    }

    let mut cursor = do_idx;
    while let Some((word, word_idx)) = prev_word_upper(tokens, cursor) {
        if word == "CONFLICT" {
            return prev_word_upper(tokens, word_idx).is_some_and(|(maybe_on, _)| maybe_on == "ON");
        }
        cursor = word_idx;
    }

    false
}

fn strip_identifier_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        return trimmed[1..trimmed.len().saturating_sub(1)].replace("]]", "]");
    }
    crate::sql_text::strip_identifier_quotes(value)
}

fn normalize_identifier_for_lookup(value: &str) -> String {
    let parts = split_identifier_parts_for_lookup(value);
    if parts.is_empty() {
        strip_identifier_quotes(value).to_ascii_uppercase()
    } else {
        parts.join(".").to_ascii_uppercase()
    }
}

fn normalize_identifier_for_scope_dedupe(value: &str) -> String {
    let trimmed = value.trim();
    if is_quoted_identifier(trimmed) {
        let unquoted = strip_identifier_quotes(trimmed);
        if unquoted.contains('.') {
            return trimmed.to_ascii_uppercase();
        }
    }
    normalize_identifier_for_lookup(value)
}

fn split_identifier_parts_for_lookup(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = value.trim().chars().peekable();
    let mut active_quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match ch {
            '"' | '`' if active_quote.is_none() || active_quote == Some(ch) => {
                current.push(ch);
                if active_quote == Some(ch) {
                    if chars.peek().copied() == Some(ch) {
                        current.push(ch);
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                } else if active_quote.is_none() {
                    active_quote = Some(ch);
                }
            }
            '[' if active_quote.is_none() => {
                current.push(ch);
                active_quote = Some(']');
            }
            ']' if active_quote == Some(']') => {
                current.push(ch);
                if chars.peek().copied() == Some(']') {
                    current.push(ch);
                    chars.next();
                } else {
                    active_quote = None;
                }
            }
            '.' if active_quote.is_none() => {
                let segment = strip_identifier_quotes(current.trim());
                if !segment.is_empty() {
                    parts.push(segment);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let segment = strip_identifier_quotes(current.trim());
    if !segment.is_empty() {
        parts.push(segment);
    }

    parts
}

fn last_identifier_part_for_lookup(value: &str) -> Option<String> {
    split_identifier_parts_for_lookup(value).into_iter().last()
}

fn is_quoted_identifier(value: &str) -> bool {
    sql_text::is_quoted_identifier(value)
}

fn is_identifier_word_token(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    if is_quoted_identifier(trimmed) {
        return !strip_identifier_quotes(trimmed).is_empty();
    }
    trimmed
        .chars()
        .next()
        .is_some_and(sql_text::is_identifier_start_char)
}

fn normalize_table_name_part(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = strip_identifier_quotes(trimmed);
    let is_quoted = is_quoted_identifier(trimmed);
    if is_quoted && unquoted.contains('.') {
        trimmed.to_string()
    } else {
        unquoted
    }
}

/// Parse a table name at the given position (handling schema.table format).
fn parse_table_name_deep(tokens: &[SqlToken], start: usize) -> Option<(String, usize)> {
    match tokens.get(start) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => None,
        Some(SqlToken::Word(word)) => {
            if let Some((wrapped_name, next_idx)) =
                parse_relation_wrapper_table_name(tokens, start, word)
            {
                return Some((wrapped_name, next_idx));
            }
            let is_quoted = is_quoted_identifier(word);
            let upper = word.to_ascii_uppercase();
            // Skip if this is a keyword rather than a table name
            if !is_quoted && (is_join_keyword(&upper) || is_table_stop_keyword(&upper)) {
                return None;
            }
            if !is_identifier_word_token(word) {
                return None;
            }
            let mut parts = vec![normalize_table_name_part(word)];
            let mut idx = start + 1;
            // Handle dotted relation names like schema.table.
            while matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == ".") {
                if let Some(SqlToken::Word(name)) = tokens.get(idx + 1) {
                    if !is_identifier_word_token(name) {
                        break;
                    }
                    parts.push(normalize_table_name_part(name));
                    idx += 2;
                    continue;
                }
                break;
            }
            let mut table = parts.join(".");

            // Handle database-link suffixes like `schema.table@remote_link`.
            if matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "@") {
                let mut dblink_idx = idx + 1;
                if let Some(SqlToken::Word(link_part)) = tokens.get(dblink_idx) {
                    if is_identifier_word_token(link_part) {
                        let mut dblink_parts = vec![normalize_table_name_part(link_part)];
                        dblink_idx += 1;

                        while matches!(tokens.get(dblink_idx), Some(SqlToken::Symbol(sym)) if sym == ".")
                        {
                            if let Some(SqlToken::Word(link_part)) = tokens.get(dblink_idx + 1) {
                                if !is_identifier_word_token(link_part) {
                                    break;
                                }
                                dblink_parts.push(normalize_table_name_part(link_part));
                                dblink_idx += 2;
                                continue;
                            }
                            break;
                        }

                        table.push('@');
                        table.push_str(&dblink_parts.join("."));
                        idx = dblink_idx;
                    }
                }
            }

            Some((table, idx))
        }
        _ => None,
    }
}

fn parse_relation_wrapper_table_name(
    tokens: &[SqlToken],
    start: usize,
    relation_word: &str,
) -> Option<(String, usize)> {
    let relation_upper = relation_word.to_ascii_uppercase();
    if relation_upper == "ROWS" {
        let from_idx = skip_comment_tokens(tokens, start + 1);
        if !matches!(tokens.get(from_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("FROM"))
        {
            return None;
        }

        let open_idx = skip_comment_tokens(tokens, from_idx + 1);
        if !matches!(tokens.get(open_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            return None;
        }

        let (_, next_idx) = extract_parenthesized_range(tokens, open_idx)?;
        return Some((relation_upper, next_idx));
    }

    if !matches!(
        relation_upper.as_str(),
        "ONLY" | "TABLE" | "THE" | "CONTAINERS" | "SHARDS"
    ) {
        return None;
    }

    let open_idx = skip_comment_tokens(tokens, start + 1);

    if relation_upper == "ONLY" {
        if matches!(tokens.get(open_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            let (inner_range, next_idx) = extract_parenthesized_range(tokens, open_idx)?;
            let inner_tokens = token_range_slice(tokens, inner_range);
            let (relation_name, _) = parse_table_name_deep(inner_tokens, 0)?;
            return Some((relation_name, next_idx));
        }

        return parse_table_name_deep(tokens, open_idx);
    }

    let Some(SqlToken::Symbol(sym)) = tokens.get(open_idx) else {
        return None;
    };
    if sym != "(" {
        return None;
    }

    let (inner_range, next_idx) = extract_parenthesized_range(tokens, open_idx)?;
    let inner_tokens = token_range_slice(tokens, inner_range);

    // TABLE/THE/CONTAINERS/SHARDS wrappers may contain collection function calls
    // or scalar subqueries. For identifier-like forms
    // (`TABLE(schema.collection_col)`, `CONTAINERS(schema.table)`) keep the
    // underlying name so alias resolution can target stable relation keys.
    if let Some((relation_name, _)) = parse_table_name_deep(inner_tokens, 0) {
        Some((relation_name, next_idx))
    } else {
        Some((relation_upper, next_idx))
    }
}

fn parse_optional_alias_column_list(
    tokens: &[SqlToken],
    start: usize,
) -> (Vec<String>, Option<TokenRange>, usize) {
    let idx = skip_comment_tokens(tokens, start);
    match tokens.get(idx) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => extract_parenthesized_range(tokens, idx)
            .map(|(range, next_idx)| {
                (
                    extract_cte_explicit_columns(tokens, range),
                    Some(range),
                    next_idx,
                )
            })
            .unwrap_or_else(|| (Vec::new(), None, idx)),
        _ => (Vec::new(), None, idx),
    }
}

fn parse_relation_alias_at_with_columns(
    tokens: &[SqlToken],
    start: usize,
    allow_alias_column_list: bool,
) -> (Option<String>, usize, Vec<String>, Option<TokenRange>) {
    let idx = skip_comment_tokens(tokens, start);
    if let Some((alias_name, after_alias)) = parse_bracket_identifier_at(tokens, idx) {
        let (explicit_columns, explicit_column_range, next_idx) = if allow_alias_column_list {
            parse_optional_alias_column_list(tokens, after_alias)
        } else {
            (Vec::new(), None, after_alias)
        };
        return (
            Some(alias_name),
            next_idx,
            explicit_columns,
            explicit_column_range,
        );
    }

    let Some(SqlToken::Word(word)) = tokens.get(idx) else {
        return (None, idx, Vec::new(), None);
    };

    let is_quoted = is_quoted_identifier(word);
    let upper = word.to_ascii_uppercase();

    if upper == "AS" {
        if matches!(next_word_upper(tokens, idx + 1), Some((next, _)) if next == "OF") {
            return (None, idx, Vec::new(), None);
        }
        let alias_idx = skip_comment_tokens(tokens, idx + 1);
        if let Some((alias_name, after_alias)) = parse_bracket_identifier_at(tokens, alias_idx) {
            let (explicit_columns, explicit_column_range, next_idx) = if allow_alias_column_list {
                parse_optional_alias_column_list(tokens, after_alias)
            } else {
                (Vec::new(), None, after_alias)
            };
            return (
                Some(alias_name),
                next_idx,
                explicit_columns,
                explicit_column_range,
            );
        }

        let Some(SqlToken::Word(alias_word)) = tokens.get(alias_idx) else {
            return (None, alias_idx, Vec::new(), None);
        };
        if !is_identifier_word_token(alias_word) {
            return (None, alias_idx + 1, Vec::new(), None);
        }
        let alias_is_quoted = is_quoted_identifier(alias_word);
        let alias_upper = alias_word.to_ascii_uppercase();
        if !alias_is_quoted && is_relation_alias_breaker(&alias_upper) {
            return (None, alias_idx, Vec::new(), None);
        }
        let (explicit_columns, explicit_column_range, next_idx) = if allow_alias_column_list {
            parse_optional_alias_column_list(tokens, alias_idx + 1)
        } else {
            (Vec::new(), None, alias_idx + 1)
        };
        return (
            Some(strip_identifier_quotes(alias_word)),
            next_idx,
            explicit_columns,
            explicit_column_range,
        );
    }

    if !is_identifier_word_token(word) {
        return (None, idx, Vec::new(), None);
    }
    if is_quoted || !is_relation_alias_breaker(&upper) {
        let (explicit_columns, explicit_column_range, next_idx) = if allow_alias_column_list {
            parse_optional_alias_column_list(tokens, idx + 1)
        } else {
            (Vec::new(), None, idx + 1)
        };
        return (
            Some(strip_identifier_quotes(word)),
            next_idx,
            explicit_columns,
            explicit_column_range,
        );
    }

    (None, idx, Vec::new(), None)
}

fn parse_bracket_identifier_at(tokens: &[SqlToken], idx: usize) -> Option<(String, usize)> {
    if !matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "[") {
        return None;
    }

    let mut inner = String::new();
    let mut cursor = idx + 1;
    while cursor < tokens.len() {
        match tokens.get(cursor)? {
            SqlToken::Symbol(sym) if sym == "]" => {
                if matches!(tokens.get(cursor + 1), Some(SqlToken::Symbol(next)) if next == "]") {
                    push_bracket_identifier_part(&mut inner, "]", false);
                    cursor += 2;
                    continue;
                }
                if inner.is_empty() {
                    return None;
                }
                return Some((inner, cursor + 1));
            }
            SqlToken::Word(word) | SqlToken::String(word) => {
                push_bracket_identifier_part(&mut inner, word, true);
            }
            SqlToken::Symbol(symbol) => {
                push_bracket_identifier_part(&mut inner, symbol, false);
            }
            SqlToken::Comment(_) => {}
        }
        cursor += 1;
    }

    None
}

fn push_bracket_identifier_part(inner: &mut String, part: &str, word_like: bool) {
    if word_like
        && !inner.is_empty()
        && inner
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    {
        inner.push(' ');
    }
    inner.push_str(part);
}

fn parse_alias_deep_with_columns(
    tokens: &[SqlToken],
    start: usize,
) -> (Option<String>, usize, Vec<String>, Option<TokenRange>) {
    let start = skip_relation_postfix_clauses(tokens, start);
    parse_relation_alias_at_with_columns(tokens, start, true)
}

fn parse_alias_after_derived_relation_clauses(
    tokens: &[SqlToken],
    start: usize,
) -> Option<(String, usize, usize, Vec<String>, Option<TokenRange>)> {
    let relation_postfix_end = skip_relation_postfix_clauses(tokens, start);
    let derived_end = skip_derived_relation_postfix_clauses(tokens, relation_postfix_end);
    if derived_end == relation_postfix_end {
        return None;
    }
    let alias_start = skip_relation_postfix_clauses(tokens, derived_end);
    let (alias, next_idx, explicit_columns, explicit_column_range) =
        parse_relation_alias_at_with_columns(tokens, alias_start, true);
    alias.map(|name| {
        (
            name,
            next_idx,
            derived_end,
            explicit_columns,
            explicit_column_range,
        )
    })
}

fn skip_relation_postfix_clauses(tokens: &[SqlToken], start: usize) -> usize {
    let mut idx = skip_comment_tokens(tokens, start);

    loop {
        let Some(SqlToken::Word(word)) = tokens.get(idx) else {
            break;
        };

        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            "USE" | "FORCE" | "IGNORE" => {
                if let Some(next_idx) = skip_mysql_index_hint_clause(tokens, idx) {
                    idx = next_idx;
                    continue;
                }
                break;
            }
            "INDEXED" => {
                let by_idx = skip_comment_tokens(tokens, idx + 1);
                if !matches!(tokens.get(by_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("BY"))
                {
                    break;
                }

                let index_name_idx = skip_comment_tokens(tokens, by_idx + 1);
                if !matches!(tokens.get(index_name_idx), Some(SqlToken::Word(index_name)) if is_identifier_word_token(index_name))
                {
                    break;
                }

                idx = skip_comment_tokens(tokens, index_name_idx + 1);
                continue;
            }
            "NOT" => {
                let indexed_idx = skip_comment_tokens(tokens, idx + 1);
                if !matches!(tokens.get(indexed_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("INDEXED"))
                {
                    break;
                }

                idx = skip_comment_tokens(tokens, indexed_idx + 1);
                continue;
            }
            "PARTITION" | "SUBPARTITION" | "SAMPLE" | "SEED" | "TABLESAMPLE" | "WITH" => {
                if upper == "WITH"
                    && matches!(
                        next_word_upper(tokens, idx + 1),
                        Some((next, _)) if next == "ORDINALITY"
                    )
                {
                    idx = skip_comment_tokens(tokens, idx + 1);
                    idx = skip_comment_tokens(tokens, idx + 1);
                    continue;
                }
                if upper == "WITH"
                    && matches!(
                        next_word_upper(tokens, idx + 1),
                        Some((next, _)) if next == "OFFSET"
                    )
                {
                    idx = skip_comment_tokens(tokens, idx + 1);
                    idx = skip_comment_tokens(tokens, idx + 1);

                    if matches!(
                        tokens.get(idx),
                        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS")
                    ) {
                        idx = skip_comment_tokens(tokens, idx + 1);
                    }

                    if matches!(tokens.get(idx), Some(SqlToken::Word(word)) if is_identifier_word_token(word))
                    {
                        idx = skip_comment_tokens(tokens, idx + 1);
                    }

                    continue;
                }
                let mut open_idx = skip_comment_tokens(tokens, idx + 1);
                if matches!(upper.as_str(), "PARTITION" | "SUBPARTITION")
                    && matches!(tokens.get(open_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("FOR"))
                {
                    open_idx = skip_comment_tokens(tokens, open_idx + 1);
                }
                if upper == "TABLESAMPLE" {
                    if matches!(tokens.get(open_idx), Some(SqlToken::Word(_))) {
                        open_idx = skip_comment_tokens(tokens, open_idx + 1);
                    }
                } else if upper == "SAMPLE"
                    && matches!(
                        tokens.get(open_idx),
                        Some(SqlToken::Word(next))
                            if next.eq_ignore_ascii_case("BLOCK")
                                || next.eq_ignore_ascii_case("BERNOULLI")
                                || next.eq_ignore_ascii_case("SYSTEM")
                    )
                {
                    open_idx = skip_comment_tokens(tokens, open_idx + 1);
                }
                if matches!(tokens.get(open_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
                    idx = skip_parenthesized_clause(tokens, open_idx);
                    if upper == "TABLESAMPLE" {
                        loop {
                            let option = next_word_upper(tokens, idx).map(|(word, _)| word);
                            let Some(option) = option else {
                                break;
                            };

                            if option != "REPEATABLE" && option != "SEED" {
                                break;
                            }

                            let option_idx = skip_comment_tokens(tokens, idx + 1);
                            if matches!(tokens.get(option_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
                            {
                                idx = skip_parenthesized_clause(tokens, option_idx);
                                continue;
                            }

                            break;
                        }
                    }
                    idx = skip_comment_tokens(tokens, idx);
                    continue;
                }
                break;
            }
            "FOR" => {
                if let Some(next_idx) = skip_relation_temporal_clause(tokens, idx) {
                    idx = skip_comment_tokens(tokens, next_idx);
                    continue;
                }
                break;
            }
            "VERSIONS" => {
                let mut between_idx = skip_comment_tokens(tokens, idx + 1);
                if matches!(tokens.get(between_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("PERIOD"))
                {
                    let for_idx = skip_comment_tokens(tokens, between_idx + 1);
                    if !matches!(tokens.get(for_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("FOR"))
                    {
                        break;
                    }
                    between_idx = skip_comment_tokens(tokens, for_idx + 1);
                    if !matches!(tokens.get(between_idx), Some(SqlToken::Word(period_name)) if is_identifier_word_token(period_name))
                    {
                        break;
                    }
                    between_idx = skip_comment_tokens(tokens, between_idx + 1);
                }

                if !matches!(tokens.get(between_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("BETWEEN"))
                {
                    break;
                }

                let and_idx = find_top_level_keyword(tokens, between_idx + 1, "AND");
                let Some(and_idx) = and_idx else {
                    break;
                };

                idx = skip_flashback_bound_expression(tokens, and_idx + 1);
                idx = skip_comment_tokens(tokens, idx);
                continue;
            }
            "AS" => {
                let of_idx = skip_comment_tokens(tokens, idx + 1);
                if !matches!(tokens.get(of_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("OF"))
                {
                    break;
                }

                let mut cursor = skip_comment_tokens(tokens, of_idx + 1);
                if matches!(tokens.get(cursor), Some(SqlToken::Word(keyword)) if keyword.eq_ignore_ascii_case("PERIOD"))
                {
                    let for_idx = skip_comment_tokens(tokens, cursor + 1);
                    if !matches!(tokens.get(for_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("FOR"))
                    {
                        break;
                    }
                    let period_name_idx = skip_comment_tokens(tokens, for_idx + 1);
                    if !matches!(tokens.get(period_name_idx), Some(SqlToken::Word(period_name)) if is_identifier_word_token(period_name))
                    {
                        break;
                    }
                    cursor = skip_comment_tokens(tokens, period_name_idx + 1);
                }

                idx = skip_flashback_bound_expression(tokens, cursor);
                idx = skip_comment_tokens(tokens, idx);
                continue;
            }
            _ => break,
        }
    }

    idx
}

fn skip_mysql_index_hint_clause(tokens: &[SqlToken], start: usize) -> Option<usize> {
    let mut idx = skip_comment_tokens(tokens, start);
    let hint_keyword = match tokens.get(idx) {
        Some(SqlToken::Word(word)) => word.to_ascii_uppercase(),
        _ => return None,
    };

    if !matches!(hint_keyword.as_str(), "USE" | "FORCE" | "IGNORE") {
        return None;
    }

    idx = skip_comment_tokens(tokens, idx + 1);
    if !matches!(tokens.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("INDEX") || word.eq_ignore_ascii_case("KEY"))
    {
        return None;
    }
    idx = skip_comment_tokens(tokens, idx + 1);

    if matches!(tokens.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("FOR")) {
        idx = skip_comment_tokens(tokens, idx + 1);
        let hint_scope = match tokens.get(idx) {
            Some(SqlToken::Word(word)) => word.to_ascii_uppercase(),
            _ => return None,
        };

        match hint_scope.as_str() {
            "JOIN" => {
                idx = skip_comment_tokens(tokens, idx + 1);
            }
            "ORDER" | "GROUP" => {
                let by_idx = skip_comment_tokens(tokens, idx + 1);
                if !matches!(tokens.get(by_idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("BY"))
                {
                    return None;
                }
                idx = skip_comment_tokens(tokens, by_idx + 1);
            }
            _ => return None,
        }
    }

    if !matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        return None;
    }

    Some(skip_comment_tokens(
        tokens,
        skip_parenthesized_clause(tokens, idx),
    ))
}

fn find_top_level_keyword(
    tokens: &[SqlToken],
    start: usize,
    target_keyword: &str,
) -> Option<usize> {
    let mut idx = start;
    let mut paren_depth = 0usize;

    while idx < tokens.len() {
        match &tokens[idx] {
            SqlToken::Symbol(sym) if sym == "(" => {
                paren_depth = paren_depth.saturating_add(1);
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            SqlToken::Word(word)
                if paren_depth == 0 && word.eq_ignore_ascii_case(target_keyword) =>
            {
                return Some(idx);
            }
            _ => {}
        }
        idx += 1;
    }

    None
}

fn skip_relation_temporal_clause(tokens: &[SqlToken], start: usize) -> Option<usize> {
    let mut idx = skip_comment_tokens(tokens, start);
    if !matches!(tokens.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("FOR")) {
        return None;
    }

    idx = skip_comment_tokens(tokens, idx + 1);
    let period_name_idx = match tokens.get(idx) {
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("SYSTEM_TIME")
                || word.eq_ignore_ascii_case("APPLICATION_TIME")
                || is_identifier_word_token(word) =>
        {
            idx
        }
        _ => return None,
    };

    idx = skip_comment_tokens(tokens, period_name_idx + 1);
    let keyword = match tokens.get(idx) {
        Some(SqlToken::Word(word)) => word.to_ascii_uppercase(),
        _ => return None,
    };

    match keyword.as_str() {
        "AS" => {
            idx = skip_comment_tokens(tokens, idx + 1);
            if !matches!(tokens.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OF"))
            {
                return None;
            }
            Some(skip_flashback_bound_expression(tokens, idx + 1))
        }
        "BETWEEN" => {
            let and_idx = find_top_level_keyword(tokens, idx + 1, "AND")?;
            Some(skip_flashback_bound_expression(tokens, and_idx + 1))
        }
        "FROM" => {
            let to_idx = find_top_level_keyword(tokens, idx + 1, "TO")?;
            Some(skip_flashback_bound_expression(tokens, to_idx + 1))
        }
        "CONTAINED" => {
            idx = skip_comment_tokens(tokens, idx + 1);
            if !matches!(tokens.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IN"))
            {
                return None;
            }
            Some(skip_flashback_bound_expression(tokens, idx + 1))
        }
        "ALL" => Some(skip_comment_tokens(tokens, idx + 1)),
        _ => None,
    }
}

fn skip_flashback_bound_expression(tokens: &[SqlToken], start: usize) -> usize {
    let mut idx = skip_comment_tokens(tokens, start);

    if matches!(
        tokens.get(idx),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("SCN")
                || word.eq_ignore_ascii_case("TIMESTAMP")
                || word.eq_ignore_ascii_case("DATE")
                || word.eq_ignore_ascii_case("SNAPSHOT")
    ) {
        idx = skip_comment_tokens(tokens, idx + 1);
    }

    // Parenthesized bound expressions are unambiguous and can be consumed in one step.
    if matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        return skip_parenthesized_clause(tokens, idx);
    }

    // Consume a simple leading operand.
    idx = consume_flashback_operand(tokens, idx);

    // Also consume optional arithmetic/interval suffixes often used in Oracle
    // flashback clauses, e.g.:
    //   AS OF TIMESTAMP SYSTIMESTAMP - INTERVAL '1' HOUR
    //   AS OF SCN 100 * 2
    //   VERSIONS BETWEEN SCN 1 * 2 AND SCN 3 * 4
    loop {
        let operator_idx = skip_comment_tokens(tokens, idx);
        let Some(SqlToken::Symbol(op)) = tokens.get(operator_idx) else {
            break;
        };
        if op != "+" && op != "-" && op != "*" && op != "/" && op != "||" {
            break;
        }

        let rhs_idx = skip_comment_tokens(tokens, operator_idx + 1);
        let Some(rhs_token) = tokens.get(rhs_idx) else {
            idx = rhs_idx;
            break;
        };

        let consumed_rhs = match rhs_token {
            SqlToken::Word(word) if word.eq_ignore_ascii_case("INTERVAL") => {
                consume_interval_literal(tokens, rhs_idx)
            }
            _ => consume_flashback_operand(tokens, rhs_idx),
        };

        if consumed_rhs == rhs_idx {
            idx = rhs_idx;
            break;
        }
        idx = consumed_rhs;
    }

    idx
}

fn consume_flashback_operand(tokens: &[SqlToken], start: usize) -> usize {
    let idx = skip_comment_tokens(tokens, start);
    match tokens.get(idx) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => skip_parenthesized_clause(tokens, idx),
        Some(SqlToken::Symbol(sym)) if sym == "+" || sym == "-" => {
            let signed_operand_idx = consume_flashback_operand(tokens, idx + 1);
            if signed_operand_idx == idx {
                idx.saturating_add(1)
            } else {
                signed_operand_idx
            }
        }
        Some(SqlToken::Word(_)) => {
            let next_idx = skip_comment_tokens(tokens, idx + 1);
            if matches!(tokens.get(next_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
                skip_parenthesized_clause(tokens, next_idx)
            } else {
                next_idx
            }
        }
        Some(SqlToken::String(_)) => skip_comment_tokens(tokens, idx + 1),
        Some(SqlToken::Symbol(sym)) if sym == "?" => skip_comment_tokens(tokens, idx + 1),
        Some(SqlToken::Symbol(sym)) if sym == ":" => {
            // Bind variable form `:b1`
            let bind_name_idx = skip_comment_tokens(tokens, idx + 1);
            if matches!(tokens.get(bind_name_idx), Some(SqlToken::Word(_))) {
                skip_comment_tokens(tokens, bind_name_idx + 1)
            } else {
                bind_name_idx
            }
        }
        Some(_) => idx.saturating_add(1),
        None => idx,
    }
}

fn consume_interval_literal(tokens: &[SqlToken], interval_idx: usize) -> usize {
    let mut idx = skip_comment_tokens(tokens, interval_idx + 1);

    if matches!(tokens.get(idx), Some(SqlToken::String(_))) {
        idx = skip_comment_tokens(tokens, idx + 1);
    }

    if matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        idx = skip_parenthesized_clause(tokens, idx);
    }

    if matches!(tokens.get(idx), Some(SqlToken::Word(_))) {
        idx = skip_comment_tokens(tokens, idx + 1);
        if matches!(tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            idx = skip_parenthesized_clause(tokens, idx);
        }
    }

    let maybe_to_idx = skip_comment_tokens(tokens, idx);
    if matches!(tokens.get(maybe_to_idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TO"))
    {
        idx = skip_comment_tokens(tokens, maybe_to_idx + 1);
        if matches!(tokens.get(idx), Some(SqlToken::Word(_))) {
            idx = skip_comment_tokens(tokens, idx + 1);
        }
    }

    idx
}

fn skip_parenthesized_clause(tokens: &[SqlToken], open_paren_idx: usize) -> usize {
    extract_parenthesized_range(tokens, open_paren_idx)
        .map(|(_, next_idx)| next_idx)
        .unwrap_or(open_paren_idx.saturating_add(1))
}

fn skip_comment_tokens(tokens: &[SqlToken], mut idx: usize) -> usize {
    while idx < tokens.len() {
        if let SqlToken::Comment(_) = &tokens[idx] {
            idx += 1;
            continue;
        }
        break;
    }
    idx
}

fn skip_derived_relation_postfix_clauses(tokens: &[SqlToken], start: usize) -> usize {
    let mut idx = start;

    loop {
        idx = skip_comment_tokens(tokens, idx);
        let Some(SqlToken::Word(word)) = tokens.get(idx) else {
            break;
        };

        let upper = word.to_ascii_uppercase();
        if upper == "MODEL" {
            idx = skip_model_clause(tokens, idx + 1);
            continue;
        }

        let clause_open_idx = match upper.as_str() {
            "PIVOT" => {
                let mut open_idx = skip_comment_tokens(tokens, idx + 1);
                if matches!(tokens.get(open_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("XML"))
                {
                    open_idx = skip_comment_tokens(tokens, open_idx + 1);
                }
                open_idx
            }
            "UNPIVOT" => {
                let mut open_idx = skip_comment_tokens(tokens, idx + 1);
                if matches!(tokens.get(open_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("INCLUDE") || next.eq_ignore_ascii_case("EXCLUDE"))
                {
                    open_idx = skip_comment_tokens(tokens, open_idx + 1);
                    if matches!(tokens.get(open_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("NULLS"))
                    {
                        open_idx = skip_comment_tokens(tokens, open_idx + 1);
                    }
                }
                open_idx
            }
            "MATCH_RECOGNIZE" => skip_comment_tokens(tokens, idx + 1),
            "MATCH" => {
                let recognize_idx = skip_comment_tokens(tokens, idx + 1);
                if !matches!(tokens.get(recognize_idx), Some(SqlToken::Word(next)) if next.eq_ignore_ascii_case("RECOGNIZE"))
                {
                    break;
                }
                skip_comment_tokens(tokens, recognize_idx + 1)
            }
            _ => break,
        };

        if !matches!(tokens.get(clause_open_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            break;
        }

        idx = extract_parenthesized_range(tokens, clause_open_idx)
            .map(|(_, next_idx)| next_idx)
            .unwrap_or(clause_open_idx.saturating_add(1));
    }

    idx
}

fn skip_model_clause(tokens: &[SqlToken], start: usize) -> usize {
    let mut idx = skip_comment_tokens(tokens, start);
    let mut saw_rules = false;
    let mut expect_rules_option_paren = false;

    while idx < tokens.len() {
        idx = skip_comment_tokens(tokens, idx);
        let Some(token) = tokens.get(idx) else {
            break;
        };

        match token {
            SqlToken::Word(word) => {
                let upper = word.to_ascii_uppercase();
                if !saw_rules {
                    if upper == "RULES" {
                        saw_rules = true;
                        idx += 1;
                        continue;
                    }
                    // Malformed MODEL clause recovery: stop when another relation
                    // boundary starts before RULES appears.
                    if is_join_keyword(&upper) || is_table_stop_keyword(&upper) {
                        break;
                    }
                    idx += 1;
                    continue;
                }

                if matches!(upper.as_str(), "ITERATE" | "UNTIL") {
                    expect_rules_option_paren = true;
                }
                idx += 1;
            }
            SqlToken::Symbol(sym) if sym == "(" => {
                let next_idx = extract_parenthesized_range(tokens, idx)
                    .map(|(_, next_idx)| next_idx)
                    .unwrap_or(idx.saturating_add(1));
                if saw_rules && !expect_rules_option_paren {
                    // First non-option parenthesized block after RULES is the
                    // model rules body; alias, if any, starts right after it.
                    return next_idx;
                }
                expect_rules_option_paren = false;
                idx = next_idx;
            }
            SqlToken::Symbol(sym) if sym == "," || sym == ")" => break,
            _ => {
                idx += 1;
            }
        }
    }

    idx
}

/// Parse an alias after a subquery closing ')' and capture any trailing derived
/// relation clauses that remain part of the virtual row source.
fn parse_subquery_alias(
    tokens: &[SqlToken],
    start: usize,
) -> Option<(String, usize, usize, Vec<String>, Option<TokenRange>)> {
    let mut idx = start;
    // Skip comments and stray closing parens to recover from malformed SQL like:
    // `FROM (SELECT ...) ) alias`
    while idx < tokens.len() {
        match &tokens[idx] {
            SqlToken::Comment(_) => {
                idx += 1;
                continue;
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                idx += 1;
                continue;
            }
            _ => {}
        }
        break;
    }

    idx = skip_relation_postfix_clauses(tokens, idx);
    let body_end = skip_derived_relation_postfix_clauses(tokens, idx);
    let alias_start = skip_relation_postfix_clauses(tokens, body_end);
    let (alias, next_idx, explicit_columns, explicit_column_range) =
        parse_relation_alias_at_with_columns(tokens, alias_start, true);
    alias.map(|name| {
        (
            name,
            next_idx,
            body_end,
            explicit_columns,
            explicit_column_range,
        )
    })
}

fn is_join_keyword(word: &str) -> bool {
    matches!(
        word,
        "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "CROSS"
            | "OUTER"
            | "NATURAL"
            | "SEMI"
            | "ANTI"
            | "ASOF"
            | "HASH"
            | "LOOP"
            | "MERGE"
            | "LATERAL"
            | "APPLY"
            | "STRAIGHT_JOIN"
    )
}

fn is_table_stop_keyword(word: &str) -> bool {
    matches!(
        word,
        "WHERE"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "CONNECT"
            | "START"
            | "WITH"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "MINUS"
            | "FETCH"
            | "FOR"
            | "WINDOW"
            | "QUALIFY"
            | "LIMIT"
            | "OFFSET"
            | "RETURNING"
            | "VALUES"
            | "SET"
            | "ON"
            | "STRAIGHT_JOIN"
            | "PIVOT"
            | "UNPIVOT"
            | "MODEL"
            | "MATCH_RECOGNIZE"
            | "MATCH"
            | "RECOGNIZE"
            | "USING"
            | "WHEN"
            | "SAMPLE"
            | "TABLESAMPLE"
            | "PARTITION"
            | "SUBPARTITION"
            | "VERSIONS"
            | "DROP"
            | "REUSE"
            | "CASCADE"
            | "RESTRICT"
            | "PURGE"
            | "STORAGE"
            | "MATERIALIZED"
            | "REJECT"
            | "UNLIMITED"
            | "TO"
    )
}

/// Keywords that must not be interpreted as relation aliases.
///
/// This is intentionally broader than `is_table_stop_keyword` and also includes
/// join modifiers such as `LATERAL` so table/subquery alias parsing follows the
/// same boundary rules.
fn is_relation_alias_breaker(word: &str) -> bool {
    is_join_keyword(word)
        || is_table_stop_keyword(word)
        || matches!(word, "SEARCH" | "CYCLE")
        || matches!(
            word,
            "ON" | "SELECT"
                | "FROM"
                | "INTO"
                | "IN"
                | "OF"
                | "LOCK"
                | "PARTITION"
                | "SUBPARTITION"
                | "VERSIONS"
        )
}

/// Collect top-level tables visible within a standalone statement.
/// This avoids full cursor-phase analysis when only table scope is needed.
pub(crate) fn collect_tables_in_statement(tokens: &[SqlToken]) -> Vec<ScopedTableRef> {
    collect_tables_deep(tokens, &[0], tokens.len()).tables
}

/// Collect tables from the scanned statement that are declared before
/// `cursor_token_len`.
///
/// This is used for JOIN-ON comparison suggestions so a not-yet-declared later
/// JOIN target (for example, `c` in
/// `JOIN tb2 b ON a.| JOIN tb3 c ON ...`) is excluded from the suggestion set.
pub(crate) fn collect_tables_in_statement_declared_before_cursor(
    tokens: &[SqlToken],
    cursor_token_len: usize,
) -> Vec<ScopedTableRef> {
    let scan_result = scan_cursor_context(tokens, cursor_token_len);
    filter_scope_entries_declared_before_cursor(&scan_result.parsed_tables, cursor_token_len)
}

fn filter_scope_entries_declared_before_cursor(
    parsed_tables: &[ParsedTableEntry],
    cursor_token_len: usize,
) -> Vec<ScopedTableRef> {
    parsed_tables
        .iter()
        .filter(|entry| entry.token_start < cursor_token_len)
        .map(|entry| entry.table.clone())
        .collect()
}

fn filter_visible_scope_entries_declared_before_cursor(
    parsed_tables: &[ParsedTableEntry],
    visible_scope_chain: &[usize],
    cursor_token_len: usize,
) -> Vec<ScopedTableRef> {
    filter_visible_scope_entries_declared_before_cursor_with_pos(
        parsed_tables,
        visible_scope_chain,
        cursor_token_len,
    )
    .into_iter()
    .map(|entry| entry.table)
    .collect()
}

fn filter_visible_scope_entries_declared_before_cursor_with_pos(
    parsed_tables: &[ParsedTableEntry],
    visible_scope_chain: &[usize],
    cursor_token_len: usize,
) -> Vec<TableRefDeclaredAtCursor> {
    let visible_scope_ids: HashSet<usize> = visible_scope_chain.iter().copied().collect();

    parsed_tables
        .iter()
        .filter(|entry| {
            visible_scope_ids.contains(&entry.scope_id) && entry.token_start < cursor_token_len
        })
        .map(|entry| TableRefDeclaredAtCursor {
            table: entry.table.clone(),
            token_start: entry.token_start,
        })
        .collect()
}

/// Resolve which tables are relevant for a given qualifier (alias or table name).
pub fn resolve_qualifier_tables(
    qualifier: &str,
    tables_in_scope: &[ScopedTableRef],
) -> Vec<String> {
    fn update_match_if_deeper(
        slot: &mut Option<(usize, String)>,
        candidate_depth: usize,
        candidate_name: &str,
    ) {
        if slot
            .as_ref()
            .is_none_or(|(depth, _)| candidate_depth >= *depth)
        {
            *slot = Some((candidate_depth, candidate_name.to_string()));
        }
    }

    fn push_first_unique(
        seen: &mut HashSet<String>,
        candidate: Option<(usize, String)>,
    ) -> Option<Vec<String>> {
        if let Some((_, name)) = candidate {
            let normalized = name.to_ascii_uppercase();
            if seen.insert(normalized) {
                return Some(vec![name]);
            }
        }
        None
    }

    let qualifier_upper = normalize_identifier_for_lookup(qualifier);
    let mut alias_match: Option<(usize, String)> = None;
    let mut name_match: Option<(usize, String)> = None;
    let mut short_name_match: Option<(usize, String)> = None;
    let mut seen = HashSet::new();

    for table_ref in tables_in_scope {
        let name_upper = normalize_identifier_for_lookup(&table_ref.name);
        let alias_upper = table_ref
            .alias
            .as_ref()
            .map(|a| normalize_identifier_for_lookup(a));

        if alias_upper.as_deref() == Some(qualifier_upper.as_str()) {
            update_match_if_deeper(&mut alias_match, table_ref.depth, &table_ref.name);
            continue;
        }

        if name_upper == qualifier_upper {
            update_match_if_deeper(&mut name_match, table_ref.depth, &table_ref.name);
            continue;
        }

        if last_identifier_part_for_lookup(&table_ref.name)
            .is_some_and(|short| short.eq_ignore_ascii_case(&qualifier_upper))
        {
            update_match_if_deeper(&mut short_name_match, table_ref.depth, &table_ref.name);
        }
    }

    if let Some(result) = push_first_unique(&mut seen, alias_match) {
        return result;
    }

    if let Some(result) = push_first_unique(&mut seen, name_match) {
        return result;
    }

    if let Some(result) = push_first_unique(&mut seen, short_name_match) {
        return result;
    }

    // If no match found, try the qualifier as a direct table name
    let qualifier_parts = split_identifier_parts_for_lookup(qualifier);
    let normalized = if qualifier_parts.is_empty() {
        strip_identifier_quotes(qualifier)
    } else {
        qualifier_parts.join(".")
    };
    if seen.insert(normalized.to_ascii_uppercase()) {
        return vec![normalized];
    }

    Vec::new()
}

/// Resolve all table names from scope (for unqualified column suggestions).
pub fn resolve_all_scope_tables(tables_in_scope: &[ScopedTableRef]) -> Vec<String> {
    let mut ordered_tables: Vec<(usize, &ScopedTableRef)> =
        tables_in_scope.iter().enumerate().collect();
    ordered_tables.sort_by(|(left_idx, left), (right_idx, right)| {
        right
            .depth
            .cmp(&left.depth)
            .then_with(|| left_idx.cmp(right_idx))
    });

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for (_, table_ref) in ordered_tables {
        let upper = normalize_identifier_for_scope_dedupe(&table_ref.name);
        if seen.insert(upper) {
            result.push(table_ref.name.clone());
        }
    }

    result
}

/// Extract projected column names from a SELECT statement's token stream.
/// Returns column names/aliases in the order they appear in the SELECT list.
/// Items that cannot be resolved (e.g., `*`, expressions without aliases) are omitted.
pub(crate) fn extract_select_list_columns(tokens: &[SqlToken]) -> Vec<String> {
    let mut columns = Vec::new();
    let select_list_tokens = extract_select_list_tokens(tokens);
    for item_tokens in split_top_level_symbol_groups(select_list_tokens, ",") {
        if let Some(col) = resolve_item_column_name(&item_tokens) {
            columns.push(col);
        }
    }

    columns
}

/// Resolve source table names referenced by wildcard items (`*`, `t.*`) in a
/// SELECT list. Returned names are deduplicated in appearance order.
// Exercised by unit tests; not yet wired into a production code path.
#[allow(dead_code)]
pub(crate) fn extract_select_list_wildcard_tables(
    tokens: &[SqlToken],
    tables_in_scope: &[ScopedTableRef],
) -> Vec<String> {
    let mut tables = Vec::new();
    let mut seen = HashSet::new();
    let select_list_tokens = extract_select_list_tokens(tokens);
    for item_tokens in split_top_level_symbol_groups(select_list_tokens, ",") {
        append_wildcard_item_tables(&item_tokens, tables_in_scope, &mut tables, &mut seen);
    }

    tables
}

/// Resolve wildcard scope names from a SELECT list while preserving aliases.
/// This is used by projection expansion, where `alias.*` and duplicate aliases
/// over the same physical table must remain distinguishable.
pub(crate) fn extract_select_list_wildcard_scopes(
    tokens: &[SqlToken],
    tables_in_scope: &[ScopedTableRef],
) -> Vec<String> {
    let mut tables = Vec::new();
    let mut seen = HashSet::new();
    let select_list_tokens = extract_select_list_tokens(tokens);
    for item_tokens in split_top_level_symbol_groups(select_list_tokens, ",") {
        append_wildcard_item_scopes(&item_tokens, tables_in_scope, &mut tables, &mut seen);
    }

    tables
}

/// Extract column names from explicit table-function output clauses such as
/// `XMLTABLE(... COLUMNS col1 NUMBER PATH '...')` or
/// `OPENJSON(... WITH (col1 int '$.id'))`.
/// Returns discovered column names in appearance order.
pub(crate) fn extract_table_function_columns(tokens: &[SqlToken]) -> Vec<String> {
    let token_depths = paren_depths(tokens);
    for (idx, token) in tokens.iter().enumerate() {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            // If this body is a normal subquery (SELECT ...), let SELECT-list
            // extraction handle it instead of mixing in function-internal tokens.
            if word.eq_ignore_ascii_case("SELECT") {
                return Vec::new();
            }
        }
    }

    let mut columns = Vec::new();
    let mut seen = HashSet::new();
    collect_table_function_columns(tokens, &mut columns, &mut seen, true);
    columns
}

/// Extract qualifiers from incomplete select-list items like `alias.`.
pub(crate) fn extract_select_list_leading_qualifiers(tokens: &[SqlToken]) -> Vec<String> {
    let mut qualifiers = Vec::new();
    let mut seen = HashSet::new();
    let select_list_tokens = extract_select_list_tokens(tokens);

    for item_tokens in split_top_level_symbol_groups(select_list_tokens, ",") {
        if let Some(qualifier) = extract_incomplete_qualified_item_prefix(&item_tokens) {
            let key = qualifier.to_ascii_uppercase();
            if seen.insert(key) {
                qualifiers.push(qualifier);
            }
        }
    }

    qualifiers
}

#[derive(Debug, Default)]
struct PivotClauseColumns {
    clause_index: usize,
    for_columns: Vec<String>,
    aggregate_columns: Vec<String>,
    generated_columns: Vec<String>,
}

#[derive(Debug, Default)]
struct UnpivotClauseColumns {
    clause_index: usize,
    source_columns: Vec<String>,
    measure_columns: Vec<String>,
    for_columns: Vec<String>,
}

#[derive(Debug, Default)]
struct ModelClauseColumns {
    measure_columns: Vec<String>,
}

/// Extract Oracle PIVOT/UNPIVOT-projected columns from a query token stream.
/// This is primarily used when the SELECT list contains `*` and normal
/// select-list extraction cannot determine output columns.
pub(crate) fn extract_oracle_pivot_unpivot_projection_columns(tokens: &[SqlToken]) -> Vec<String> {
    let pivot = parse_top_level_pivot_clause(tokens);
    let unpivot = parse_top_level_unpivot_clause(tokens);
    if pivot.is_none() && unpivot.is_none() {
        return Vec::new();
    }

    let mut columns = extract_oracle_pivot_unpivot_source_projection_columns(tokens);

    if let Some(pivot_info) = pivot {
        remove_columns_case_insensitive(&mut columns, &pivot_info.for_columns);
        remove_columns_case_insensitive(&mut columns, &pivot_info.aggregate_columns);
        columns.extend(pivot_info.generated_columns);
    }

    if let Some(unpivot_info) = unpivot {
        remove_columns_case_insensitive(&mut columns, &unpivot_info.source_columns);
        columns.extend(unpivot_info.measure_columns);
        columns.extend(unpivot_info.for_columns);
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

pub(crate) fn extract_oracle_pivot_unpivot_source_projection_columns(
    tokens: &[SqlToken],
) -> Vec<String> {
    let pivot = parse_top_level_pivot_clause(tokens);
    let unpivot = parse_top_level_unpivot_clause(tokens);
    let Some(clause_idx) = first_pivot_unpivot_clause_index(pivot.as_ref(), unpivot.as_ref())
    else {
        return Vec::new();
    };

    let mut columns = infer_source_columns_before_clause(tokens, clause_idx);
    dedup_columns_case_insensitive(&mut columns);
    columns
}

/// Extract Oracle UNPIVOT-introduced columns (measure + FOR target).
pub(crate) fn extract_oracle_unpivot_generated_columns(tokens: &[SqlToken]) -> Vec<String> {
    let Some(unpivot_info) = parse_top_level_unpivot_clause(tokens) else {
        return Vec::new();
    };

    let mut columns = unpivot_info.measure_columns;
    columns.extend(unpivot_info.for_columns);
    dedup_columns_case_insensitive(&mut columns);
    columns
}

/// Extract Oracle MODEL-introduced measure columns from `MEASURES (...)`.
pub(crate) fn extract_oracle_model_generated_columns(tokens: &[SqlToken]) -> Vec<String> {
    let Some(model_info) = parse_top_level_model_clause(tokens) else {
        return Vec::new();
    };
    model_info.measure_columns
}

/// Extract recursive CTE-generated columns introduced by trailing SEARCH/CYCLE
/// clauses after the CTE body.
pub(crate) fn extract_recursive_cte_generated_columns(
    tokens: &[SqlToken],
    cte_body_end: usize,
) -> Vec<String> {
    let mut idx = skip_comment_tokens(tokens, cte_body_end.saturating_add(1));
    let mut columns = Vec::new();

    loop {
        let Some((keyword, keyword_idx)) = next_word_upper(tokens, idx) else {
            break;
        };

        match keyword.as_str() {
            "SEARCH" => {
                let Some((column, next_idx)) =
                    parse_recursive_cte_search_generated_column(tokens, keyword_idx)
                else {
                    break;
                };
                columns.push(column);
                idx = next_idx;
            }
            "CYCLE" => {
                let Some((column, next_idx)) =
                    parse_recursive_cte_cycle_generated_column(tokens, keyword_idx)
                else {
                    break;
                };
                columns.push(column);
                idx = next_idx;
            }
            _ => break,
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

/// Extract MATCH_RECOGNIZE-generated columns from a query token stream.
/// This includes MEASURES aliases and PATTERN/SUBSET variables.
pub(crate) fn extract_match_recognize_generated_columns(tokens: &[SqlToken]) -> Vec<String> {
    let mut columns = extract_match_recognize_measure_columns(tokens);
    columns.extend(extract_match_recognize_pattern_variables(tokens));
    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn extract_match_recognize_measure_columns(tokens: &[SqlToken]) -> Vec<String> {
    let Some(clause_tokens) = extract_match_recognize_clause_tokens(tokens) else {
        return Vec::new();
    };
    let token_depths = paren_depths(clause_tokens);

    let mut measures_idx = None;
    for (idx, token) in clause_tokens.iter().enumerate() {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            if word.eq_ignore_ascii_case("MEASURES") {
                measures_idx = Some(idx);
                break;
            }
        }
    }

    let Some(measures_idx) = measures_idx else {
        return Vec::new();
    };

    let measures_start = next_non_comment_index(clause_tokens, measures_idx.saturating_add(1));
    if measures_start >= clause_tokens.len() {
        return Vec::new();
    }

    let mut measures_end = clause_tokens.len();
    for (idx, token) in clause_tokens.iter().enumerate().skip(measures_start) {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            let upper = word.to_ascii_uppercase();
            if is_match_recognize_clause_boundary_keyword(&upper) {
                measures_end = idx;
                break;
            }
        }
    }

    let mut columns = Vec::new();
    for item_tokens in
        split_top_level_symbol_groups(&clause_tokens[measures_start..measures_end], ",")
    {
        if let Some(column) = parse_model_measure_output_column(&item_tokens) {
            columns.push(column);
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

/// Extract MATCH_RECOGNIZE pattern variables from `PATTERN (...)` and
/// subset variables from `SUBSET ...`.
/// Example: `PATTERN (a b+) SUBSET u = (a, b)` -> `["a", "b", "u"]`.
pub(crate) fn extract_match_recognize_pattern_variables(tokens: &[SqlToken]) -> Vec<String> {
    let Some(clause_tokens) = extract_match_recognize_clause_tokens(tokens) else {
        return Vec::new();
    };
    let token_depths = paren_depths(clause_tokens);

    let mut pattern_idx = None;
    for (idx, token) in clause_tokens.iter().enumerate() {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            if word.eq_ignore_ascii_case("PATTERN") {
                pattern_idx = Some(idx);
                break;
            }
        }
    }

    let Some(pattern_idx) = pattern_idx else {
        return Vec::new();
    };
    let pattern_open_idx = next_non_comment_index(clause_tokens, pattern_idx.saturating_add(1));
    let Some(SqlToken::Symbol(sym)) = clause_tokens.get(pattern_open_idx) else {
        return Vec::new();
    };
    if sym != "(" {
        return Vec::new();
    }

    let Some((pattern_range, _)) = extract_parenthesized_range(clause_tokens, pattern_open_idx)
    else {
        return Vec::new();
    };

    let mut variables = Vec::new();
    let pattern_tokens = token_range_slice(clause_tokens, pattern_range);
    for token in pattern_tokens {
        if let SqlToken::Word(word) = token {
            if !is_identifier_word_token(word) {
                continue;
            }
            let upper = word.to_ascii_uppercase();
            if is_match_recognize_pattern_keyword(&upper) {
                continue;
            }
            variables.push(output_identifier_suggestion(word));
        }
    }

    if let Some(subset_idx) = clause_tokens.iter().enumerate().find_map(|(idx, token)| {
        if !is_top_level_depth(&token_depths, idx) {
            return None;
        }

        match token {
            SqlToken::Word(word) if word.eq_ignore_ascii_case("SUBSET") => Some(idx),
            _ => None,
        }
    }) {
        let mut idx = next_non_comment_index(clause_tokens, subset_idx.saturating_add(1));
        while idx < clause_tokens.len() {
            if let Some(SqlToken::Word(word)) = clause_tokens.get(idx) {
                let upper = word.to_ascii_uppercase();
                if is_match_recognize_clause_boundary_keyword(&upper) {
                    break;
                }

                let assign_idx = next_non_comment_index(clause_tokens, idx.saturating_add(1));
                if is_identifier_word_token(word)
                    && matches!(clause_tokens.get(assign_idx), Some(SqlToken::Symbol(sym)) if sym == "=")
                {
                    variables.push(output_identifier_suggestion(word));

                    let rhs_idx = next_non_comment_index(clause_tokens, assign_idx + 1);
                    idx = if matches!(clause_tokens.get(rhs_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
                    {
                        extract_parenthesized_range(clause_tokens, rhs_idx)
                            .map(|(_, next_idx)| next_idx)
                            .unwrap_or(rhs_idx.saturating_add(1))
                    } else {
                        rhs_idx.saturating_add(1)
                    };
                } else {
                    idx = idx.saturating_add(1);
                }
            } else {
                idx = idx.saturating_add(1);
            }

            idx = next_non_comment_index(clause_tokens, idx);
            if matches!(clause_tokens.get(idx), Some(SqlToken::Symbol(sym)) if sym == ",") {
                idx = next_non_comment_index(clause_tokens, idx + 1);
            }
        }
    }

    dedup_columns_case_insensitive(&mut variables);
    variables
}

fn parse_recursive_cte_search_generated_column(
    tokens: &[SqlToken],
    search_idx: usize,
) -> Option<(String, usize)> {
    let (mode, mode_idx) = next_word_upper(tokens, search_idx.saturating_add(1))?;
    if !matches!(mode.as_str(), "DEPTH" | "BREADTH") {
        return None;
    }

    let (first, first_idx) = next_word_upper(tokens, mode_idx.saturating_add(1))?;
    if first != "FIRST" {
        return None;
    }

    let (by, by_idx) = next_word_upper(tokens, first_idx.saturating_add(1))?;
    if by != "BY" {
        return None;
    }

    find_recursive_cte_generated_column_after_set(tokens, by_idx.saturating_add(1))
}

fn parse_recursive_cte_cycle_generated_column(
    tokens: &[SqlToken],
    cycle_idx: usize,
) -> Option<(String, usize)> {
    find_recursive_cte_generated_column_after_set(tokens, cycle_idx.saturating_add(1))
}

fn find_recursive_cte_generated_column_after_set(
    tokens: &[SqlToken],
    start_idx: usize,
) -> Option<(String, usize)> {
    let mut idx = skip_comment_tokens(tokens, start_idx);

    while idx < tokens.len() {
        match tokens.get(idx) {
            Some(SqlToken::Comment(_)) => {
                idx += 1;
            }
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("SET") => {
                let column_idx = next_non_comment_index(tokens, idx.saturating_add(1));
                let SqlToken::Word(column_name) = tokens.get(column_idx)? else {
                    return None;
                };
                if !is_identifier_word_token(column_name) {
                    return None;
                }
                return Some((
                    output_identifier_suggestion(column_name),
                    skip_comment_tokens(tokens, column_idx.saturating_add(1)),
                ));
            }
            Some(SqlToken::Word(word))
                if sql_text::is_with_main_query_keyword(&word.to_ascii_uppercase()) =>
            {
                return None;
            }
            Some(_) => {
                idx += 1;
            }
            None => break,
        }
    }

    None
}

fn extract_match_recognize_clause_tokens(tokens: &[SqlToken]) -> Option<&[SqlToken]> {
    let match_idx = find_top_level_word_index(tokens, "MATCH_RECOGNIZE")
        .or_else(|| find_top_level_keyword_pair_index(tokens, "MATCH", "RECOGNIZE"))?;

    let mut clause_start_idx = match_idx.saturating_add(1);
    if let Some((next_keyword, next_idx)) = next_word_upper(tokens, clause_start_idx) {
        if next_keyword == "RECOGNIZE" {
            clause_start_idx = next_idx.saturating_add(1);
        }
    }

    let clause_open_idx = next_non_comment_index(tokens, clause_start_idx);
    let SqlToken::Symbol(sym) = tokens.get(clause_open_idx)? else {
        return None;
    };
    if sym != "(" {
        return None;
    }

    let (clause_range, _) = extract_parenthesized_range(tokens, clause_open_idx)?;
    Some(token_range_slice(tokens, clause_range))
}

fn is_match_recognize_clause_boundary_keyword(word: &str) -> bool {
    matches!(
        word,
        "MEASURES" | "PATTERN" | "DEFINE" | "AFTER" | "SUBSET" | "ONE" | "ALL" | "WITH"
    )
}

fn find_top_level_keyword_pair_index(
    tokens: &[SqlToken],
    first: &str,
    second: &str,
) -> Option<usize> {
    let mut paren_state = ParenDepthState::default();

    for (idx, token) in tokens.iter().enumerate() {
        apply_paren_token(&mut paren_state, token);
        if paren_state.depth() != 0 {
            continue;
        }
        let SqlToken::Word(word) = token else {
            continue;
        };
        if !word.eq_ignore_ascii_case(first) {
            continue;
        }
        if top_level_next_word_is(tokens, idx.saturating_add(1), second) {
            return Some(idx);
        }
    }

    None
}

fn top_level_next_word_is(tokens: &[SqlToken], start_idx: usize, keyword: &str) -> bool {
    let mut idx = start_idx;
    let mut paren_state = ParenDepthState::default();

    while idx < tokens.len() {
        match tokens.get(idx) {
            Some(SqlToken::Comment(_)) => {
                idx += 1;
                continue;
            }
            Some(token) => {
                apply_paren_token(&mut paren_state, token);
                if paren_state.depth() != 0 {
                    idx += 1;
                    continue;
                }

                if let SqlToken::Word(word) = token {
                    return word.eq_ignore_ascii_case(keyword);
                }
                return false;
            }
            None => break,
        }
    }

    false
}

fn first_pivot_unpivot_clause_index(
    pivot: Option<&PivotClauseColumns>,
    unpivot: Option<&UnpivotClauseColumns>,
) -> Option<usize> {
    match (pivot, unpivot) {
        (Some(p), Some(u)) => Some(p.clause_index.min(u.clause_index)),
        (Some(p), None) => Some(p.clause_index),
        (None, Some(u)) => Some(u.clause_index),
        (None, None) => None,
    }
}

fn infer_source_columns_before_clause(tokens: &[SqlToken], clause_idx: usize) -> Vec<String> {
    if let Some(source_range) = extract_preceding_parenthesized_range(tokens, clause_idx) {
        let columns = infer_projection_columns_from_query_tokens_allowing_pivot(
            token_range_slice(tokens, source_range),
            true,
        );
        if !columns.is_empty() {
            return columns;
        }
    }

    let analysis = collect_tables_deep(tokens, &[0], tokens.len());
    let mut selected_subquery: Option<&SubqueryDefinition> = None;

    for subq in &analysis.subqueries {
        if subq.depth != 0 || subq.body_range.end > clause_idx {
            continue;
        }
        if selected_subquery
            .as_ref()
            .is_none_or(|existing| subq.body_range.end > existing.body_range.end)
        {
            selected_subquery = Some(subq);
        }
    }

    if let Some(subq) = selected_subquery {
        let body_tokens = token_range_slice(tokens, subq.body_range);
        return infer_projection_columns_from_query_tokens(body_tokens);
    }

    infer_projection_columns_from_query_tokens_allowing_pivot(tokens, false)
}

fn infer_projection_columns_from_query_tokens(tokens: &[SqlToken]) -> Vec<String> {
    infer_projection_columns_from_query_tokens_allowing_pivot(tokens, true)
}

fn infer_projection_columns_from_query_tokens_allowing_pivot(
    tokens: &[SqlToken],
    allow_pivot: bool,
) -> Vec<String> {
    let mut columns = extract_select_list_columns(tokens);
    if columns.is_empty() {
        columns = extract_table_function_columns(tokens);
    }
    if allow_pivot && columns.is_empty() {
        columns = extract_oracle_pivot_unpivot_projection_columns(tokens);
    }
    if columns.is_empty() {
        columns = extract_oracle_model_generated_columns(tokens);
    }
    if columns.is_empty() {
        columns = extract_match_recognize_generated_columns(tokens);
    }
    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_top_level_pivot_clause(tokens: &[SqlToken]) -> Option<PivotClauseColumns> {
    let pivot_idx = find_top_level_word_index(tokens, "PIVOT")?;
    let mut idx = next_non_comment_index(tokens, pivot_idx.saturating_add(1));
    let mut pivot_mode = PivotMode::Regular;

    if let Some(SqlToken::Word(word)) = tokens.get(idx) {
        if word.eq_ignore_ascii_case("XML") {
            pivot_mode = PivotMode::Xml;
            idx = next_non_comment_index(tokens, idx.saturating_add(1));
        }
    }

    let open_idx = match tokens.get(idx) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => idx,
        _ => return None,
    };

    let (range, _) = extract_parenthesized_range(tokens, open_idx)?;
    let clause_tokens = token_range_slice(tokens, range);
    let (for_idx, in_idx) = find_clause_for_in_indices(clause_tokens)?;

    let aggregate_columns = parse_pivot_aggregate_columns(&clause_tokens[..for_idx]);
    let aggregate_aliases = parse_pivot_aggregate_aliases(&clause_tokens[..for_idx]);
    let for_columns = parse_identifier_segment(&clause_tokens[for_idx + 1..in_idx]);
    let generated_columns = if pivot_mode.should_skip_generated_columns() {
        Vec::new()
    } else {
        parse_pivot_generated_columns_from_in_segment(
            &clause_tokens[in_idx + 1..],
            &aggregate_aliases,
        )
    };

    let mut result = PivotClauseColumns {
        clause_index: pivot_idx,
        for_columns,
        aggregate_columns,
        generated_columns,
    };
    dedup_columns_case_insensitive(&mut result.for_columns);
    dedup_columns_case_insensitive(&mut result.aggregate_columns);
    dedup_columns_case_insensitive(&mut result.generated_columns);
    Some(result)
}

fn parse_top_level_unpivot_clause(tokens: &[SqlToken]) -> Option<UnpivotClauseColumns> {
    let unpivot_idx = find_top_level_word_index(tokens, "UNPIVOT")?;
    let mut idx = next_non_comment_index(tokens, unpivot_idx.saturating_add(1));

    if let Some(SqlToken::Word(word)) = tokens.get(idx) {
        if word.eq_ignore_ascii_case("INCLUDE") || word.eq_ignore_ascii_case("EXCLUDE") {
            idx = next_non_comment_index(tokens, idx.saturating_add(1));
            if let Some(SqlToken::Word(nulls)) = tokens.get(idx) {
                if nulls.eq_ignore_ascii_case("NULLS") {
                    idx = next_non_comment_index(tokens, idx.saturating_add(1));
                }
            }
        }
    }

    let open_idx = match tokens.get(idx) {
        Some(SqlToken::Symbol(sym)) if sym == "(" => idx,
        _ => return None,
    };

    let (range, _) = extract_parenthesized_range(tokens, open_idx)?;
    let clause_tokens = token_range_slice(tokens, range);
    let (for_idx, in_idx) = find_clause_for_in_indices(clause_tokens)?;

    let measure_columns = parse_unpivot_output_segment(&clause_tokens[..for_idx]);
    let for_columns = parse_unpivot_output_segment(&clause_tokens[for_idx + 1..in_idx]);
    let source_columns = parse_unpivot_source_columns_from_in_segment(&clause_tokens[in_idx + 1..]);

    let mut result = UnpivotClauseColumns {
        clause_index: unpivot_idx,
        source_columns,
        measure_columns,
        for_columns,
    };
    dedup_columns_case_insensitive(&mut result.source_columns);
    dedup_columns_case_insensitive(&mut result.measure_columns);
    dedup_columns_case_insensitive(&mut result.for_columns);
    Some(result)
}

fn parse_top_level_model_clause(tokens: &[SqlToken]) -> Option<ModelClauseColumns> {
    let model_idx = find_top_level_word_index(tokens, "MODEL")?;
    let token_depths = paren_depths(tokens);
    let mut idx = model_idx.saturating_add(1);

    while idx < tokens.len() {
        if !is_top_level_depth(&token_depths, idx) {
            idx += 1;
            continue;
        }
        if let SqlToken::Word(word) = &tokens[idx] {
            if !word.eq_ignore_ascii_case("MEASURES") {
                idx += 1;
                continue;
            }

            let open_idx = next_non_comment_index(tokens, idx.saturating_add(1));
            let Some(SqlToken::Symbol(sym)) = tokens.get(open_idx) else {
                return None;
            };
            if sym != "(" {
                return None;
            }

            let (measure_range, _) = extract_parenthesized_range(tokens, open_idx)?;
            let mut result = ModelClauseColumns {
                measure_columns: parse_model_measure_columns(token_range_slice(
                    tokens,
                    measure_range,
                )),
            };
            dedup_columns_case_insensitive(&mut result.measure_columns);
            return Some(result);
        }
        idx += 1;
    }

    None
}

fn find_top_level_word_index(tokens: &[SqlToken], keyword: &str) -> Option<usize> {
    let token_depths = paren_depths(tokens);
    let mut idx = 0usize;
    while idx < tokens.len() {
        if !is_top_level_depth(&token_depths, idx) {
            idx += 1;
            continue;
        }
        if let SqlToken::Word(word) = &tokens[idx] {
            if word.eq_ignore_ascii_case(keyword) {
                return Some(idx);
            }
        }
        idx += 1;
    }
    None
}

fn find_clause_for_in_indices(clause_tokens: &[SqlToken]) -> Option<(usize, usize)> {
    let token_depths = paren_depths(clause_tokens);
    let mut for_idx = None;
    let mut in_idx = None;
    let mut idx = 0usize;

    while idx < clause_tokens.len() {
        if !is_top_level_depth(&token_depths, idx) {
            idx += 1;
            continue;
        }
        if let SqlToken::Word(word) = &clause_tokens[idx] {
            if for_idx.is_none() && word.eq_ignore_ascii_case("FOR") {
                for_idx = Some(idx);
            } else if for_idx.is_some() && word.eq_ignore_ascii_case("IN") {
                in_idx = Some(idx);
                break;
            }
        }
        idx += 1;
    }

    match (for_idx, in_idx) {
        (Some(for_pos), Some(in_pos)) if for_pos < in_pos => Some((for_pos, in_pos)),
        _ => None,
    }
}

fn parse_pivot_aggregate_columns(tokens: &[SqlToken]) -> Vec<String> {
    let mut columns = Vec::new();
    let mut idx = 0usize;

    while idx < tokens.len() {
        let token = &tokens[idx];
        if let SqlToken::Word(func_name) = token {
            if !is_identifier_word_token(func_name) {
                idx += 1;
                continue;
            }

            let open_idx = next_non_comment_index(tokens, idx.saturating_add(1));
            let Some(SqlToken::Symbol(sym)) = tokens.get(open_idx) else {
                idx += 1;
                continue;
            };
            if sym != "(" {
                idx += 1;
                continue;
            }

            if let Some((args_range, next_idx)) = extract_parenthesized_range(tokens, open_idx) {
                let args_tokens = token_range_slice(tokens, args_range);
                let is_listagg = func_name.eq_ignore_ascii_case("LISTAGG");
                for arg_item in split_top_level_symbol_groups(args_tokens, ",") {
                    columns.extend(parse_identifiers_from_expression_tokens_with_context(
                        &arg_item, is_listagg,
                    ));
                }
                idx = next_idx;
                continue;
            }
        }

        idx += 1;
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_pivot_aggregate_aliases(tokens: &[SqlToken]) -> Vec<String> {
    let mut aliases = Vec::new();

    for item_tokens in split_top_level_symbol_groups(tokens, ",") {
        if let Some(alias) = parse_pivot_aggregate_alias_from_item(&item_tokens) {
            aliases.push(alias);
        }
    }

    dedup_columns_case_insensitive(&mut aliases);
    aliases
}

fn parse_pivot_aggregate_alias_from_item(item_tokens: &[&SqlToken]) -> Option<String> {
    let mut depth = 0usize;
    let mut first_call_close_idx = None;
    let mut idx = 0usize;

    while idx < item_tokens.len() {
        match item_tokens[idx] {
            SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
            SqlToken::Symbol(sym) if sym == ")" => {
                depth = depth.saturating_sub(1);
                if depth == 0 && first_call_close_idx.is_none() {
                    first_call_close_idx = Some(idx);
                }
            }
            SqlToken::Word(word) if depth == 0 && word.eq_ignore_ascii_case("AS") => {
                return parse_next_identifier_from_token_refs(item_tokens, idx.saturating_add(1));
            }
            SqlToken::Word(word)
                if depth == 0
                    && first_call_close_idx.is_some_and(|close_idx| idx > close_idx)
                    && is_identifier_word_token(word)
                    && !is_expression_keyword(&word.to_ascii_uppercase()) =>
            {
                let next_is_call = matches!(
                    item_tokens.get(idx.saturating_add(1)),
                    Some(SqlToken::Symbol(sym)) if sym == "("
                );
                if !next_is_call {
                    return Some(output_identifier_suggestion(word));
                }
            }
            _ => {}
        }
        idx += 1;
    }

    None
}

fn parse_next_identifier_from_token_refs(tokens: &[&SqlToken], start: usize) -> Option<String> {
    let mut idx = start.min(tokens.len());
    while idx < tokens.len() {
        match tokens[idx] {
            SqlToken::Comment(_) => idx += 1,
            SqlToken::Word(alias) if is_identifier_word_token(alias) => {
                return Some(output_identifier_suggestion(alias));
            }
            _ => return None,
        }
    }
    None
}

fn parse_identifiers_from_expression_tokens_with_context(
    tokens: &[&SqlToken],
    is_listagg_arg: bool,
) -> Vec<String> {
    let meaningful: Vec<&SqlToken> = tokens
        .iter()
        .copied()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect();
    if meaningful.is_empty() {
        return Vec::new();
    }

    let mut columns = Vec::new();
    let token_depths = paren_depths_for_token_refs(&meaningful);

    for (idx, token) in meaningful.iter().enumerate() {
        let SqlToken::Word(word) = token else {
            continue;
        };
        if !is_identifier_word_token(word) {
            continue;
        }

        let upper = word.to_ascii_uppercase();
        if is_expression_keyword(&upper) {
            continue;
        }

        if is_cast_type_spec_identifier(&meaningful, &token_depths, idx, &upper)
            || is_cast_type_name_path_identifier(&meaningful, &token_depths, idx)
            || is_cast_type_length_semantics_keyword(&meaningful, &token_depths, idx, &upper)
            || is_cast_character_set_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_extract_datetime_field_identifier(&meaningful, &token_depths, idx, &upper)
            || is_extract_from_keyword(&meaningful, &token_depths, idx, &upper)
            || is_trim_syntax_keyword(&meaningful, &token_depths, idx, &upper)
            || is_substring_syntax_keyword(&meaningful, &token_depths, idx, &upper)
            || is_from_consuming_function_syntax_keyword(&meaningful, &token_depths, idx, &upper)
            || is_datetime_literal_syntax_word(&meaningful, idx, &upper)
            || is_json_function_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_json_predicate_syntax_word(&meaningful, &token_depths, idx, &upper)
            || (is_listagg_arg
                && is_listagg_overflow_syntax_word(&meaningful, &token_depths, idx, &upper))
            || is_xmlparse_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_xmlroot_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_xmlserialize_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_xml_generation_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_is_of_type_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_member_of_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_submultiset_of_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_multiset_operator_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_collection_condition_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_sounds_like_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_like_escape_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_numeric_condition_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_boolean_condition_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_distinct_from_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_window_frame_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_analytic_null_treatment_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_nth_value_from_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_xmlquery_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_at_time_zone_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_collate_syntax_word(&meaningful, &token_depths, idx, &upper)
            || is_treat_type_spec_identifier(&meaningful, &token_depths, idx)
            || is_conversion_default_syntax_word(&meaningful, &token_depths, idx, &upper)
        {
            continue;
        }

        let prev_is_bind_marker = matches!(idx.checked_sub(1).and_then(|prev_idx| meaningful.get(prev_idx)), Some(SqlToken::Symbol(sym)) if sym == ":");
        if prev_is_bind_marker {
            continue;
        }

        let next_is_named_arg_arrow = matches!(
            meaningful.get(idx + 1),
            Some(SqlToken::Symbol(sym)) if sym == "=>"
        ) || (matches!(meaningful.get(idx + 1), Some(SqlToken::Symbol(sym)) if sym == "=")
            && matches!(meaningful.get(idx + 2), Some(SqlToken::Symbol(sym)) if sym == ">"));
        if next_is_named_arg_arrow {
            continue;
        }

        let next_is_dot =
            matches!(meaningful.get(idx + 1), Some(SqlToken::Symbol(sym)) if sym == ".");
        if next_is_dot {
            continue;
        }

        let next_is_call =
            matches!(meaningful.get(idx + 1), Some(SqlToken::Symbol(sym)) if sym == "(");
        if next_is_call {
            continue;
        }

        columns.push(strip_identifier_quotes(word));
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn paren_depths_for_token_refs(tokens: &[&SqlToken]) -> Vec<usize> {
    let mut depth = 0usize;
    let mut depths = Vec::with_capacity(tokens.len());

    for token in tokens {
        match token {
            SqlToken::Symbol(sym) if sym == "(" => {
                depths.push(depth);
                depth = depth.saturating_add(1);
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                depth = depth.saturating_sub(1);
                depths.push(depth);
            }
            _ => depths.push(depth),
        }
    }

    depths
}

fn is_cast_type_spec_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !is_sql_type_spec_word(word_upper) {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    let mut as_idx = None;
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return false;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS") => {
                as_idx = Some(scan_idx);
                break;
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return false,
            _ => {}
        }
    }

    let Some(as_idx) = as_idx else {
        return false;
    };

    let mut scan_idx = as_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return false;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let Some(call_idx) = scan_idx.checked_sub(1) else {
                return false;
            };
            return matches!(
                tokens.get(call_idx).copied(),
                Some(SqlToken::Word(call_name))
                    if call_name.eq_ignore_ascii_case("CAST")
                        || call_name.eq_ignore_ascii_case("CONVERT")
                        || call_name.eq_ignore_ascii_case("XMLCAST")
                        || call_name.eq_ignore_ascii_case("XMLSERIALIZE")
                        || call_name.eq_ignore_ascii_case("VALIDATE_CONVERSION")
            );
        }
    }

    false
}

fn is_cast_type_name_path_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    let Some(as_idx) = cast_type_as_index_before(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !cast_like_call_before_as(tokens, token_depths, as_idx, depth) {
        return false;
    }

    type_name_path_contains_identifier(tokens, token_depths, as_idx, idx, depth)
}

fn cast_type_as_index_before(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS") => {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn cast_like_call_before_as(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    as_idx: usize,
    depth: usize,
) -> bool {
    let mut scan_idx = as_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return false;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let Some(call_idx) = scan_idx.checked_sub(1) else {
                return false;
            };
            return matches!(
                tokens.get(call_idx).copied(),
                Some(SqlToken::Word(call_name))
                    if call_name.eq_ignore_ascii_case("CAST")
                        || call_name.eq_ignore_ascii_case("CONVERT")
                        || call_name.eq_ignore_ascii_case("XMLCAST")
                        || call_name.eq_ignore_ascii_case("XMLSERIALIZE")
                        || call_name.eq_ignore_ascii_case("VALIDATE_CONVERSION")
            );
        }
    }

    false
}

fn type_name_path_contains_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    as_idx: usize,
    idx: usize,
    depth: usize,
) -> bool {
    let mut scan_idx = as_idx.saturating_add(1);
    let mut expect_word = true;
    let mut saw_word = false;

    while scan_idx <= idx {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth != depth {
            return false;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Word(word)) if expect_word && is_identifier_word_token(word) => {
                saw_word = true;
                if scan_idx == idx {
                    return true;
                }
                expect_word = false;
            }
            Some(SqlToken::Symbol(sym)) if saw_word && !expect_word && sym == "." => {
                expect_word = true;
            }
            _ => return false,
        }

        scan_idx += 1;
    }

    false
}

fn is_cast_type_length_semantics_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "CHAR" | "BYTE") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    let Some(type_open_idx) =
        previous_open_paren_index_at_depth(tokens, token_depths, idx, depth.saturating_sub(1))
    else {
        return false;
    };
    let Some(type_word_idx) =
        previous_word_index_at_depth(tokens, token_depths, type_open_idx, depth.saturating_sub(1))
    else {
        return false;
    };

    matches!(
        tokens.get(type_word_idx).copied(),
        Some(SqlToken::Word(type_word))
            if matches!(
                type_word.to_ascii_uppercase().as_str(),
                "CHAR" | "CHARACTER" | "VARCHAR" | "VARCHAR2" | "NCHAR" | "NVARCHAR2"
            ) && is_cast_type_spec_identifier(
                tokens,
                token_depths,
                type_word_idx,
                &type_word.to_ascii_uppercase(),
            )
    )
}

fn is_cast_character_set_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    if word_upper == "SET" {
        let Some(character_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
        else {
            return false;
        };
        return matches!(
            tokens.get(character_idx).copied(),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("CHARACTER")
                    && is_cast_type_spec_identifier(tokens, token_depths, character_idx, "CHARACTER")
        );
    }

    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|set_idx| {
        matches!(
            tokens.get(set_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("SET")
        ) && is_cast_character_set_syntax_word(tokens, token_depths, set_idx, "SET")
    })
}

fn previous_open_paren_index_at_depth(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        if matches!(tokens.get(scan_idx).copied(), Some(SqlToken::Symbol(sym)) if sym == "(") {
            return Some(scan_idx);
        }
    }

    None
}

fn is_treat_type_spec_identifier(tokens: &[&SqlToken], token_depths: &[usize], idx: usize) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || call_open_index(tokens, token_depths, idx, depth, "TREAT").is_none() {
        return false;
    }

    keyword_before_in_same_call(tokens, token_depths, idx, depth, "AS").is_some()
}

fn is_extract_datetime_field_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !is_extract_datetime_field_word(word_upper) {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    let Some(open_idx) = call_open_index(tokens, token_depths, idx, depth, "EXTRACT") else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    for scan_idx in open_idx.saturating_add(1)..idx {
        if token_depths.get(scan_idx).copied() != Some(depth) {
            continue;
        }
        if matches!(
            tokens.get(scan_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("FROM")
        ) {
            return false;
        }
    }

    true
}

fn is_extract_from_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if word_upper != "FROM" {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    depth > 0 && call_open_index(tokens, token_depths, idx, depth, "EXTRACT").is_some()
}

fn call_open_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    call_name: &str,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return None;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let call_idx = scan_idx.checked_sub(1)?;
            return matches!(
                tokens.get(call_idx).copied(),
                Some(SqlToken::Word(actual_call_name))
                    if actual_call_name.eq_ignore_ascii_case(call_name)
            )
            .then_some(scan_idx);
        }
    }

    None
}

fn is_extract_datetime_field_word(word: &str) -> bool {
    matches!(
        word,
        "YEAR"
            | "MONTH"
            | "DAY"
            | "HOUR"
            | "MINUTE"
            | "SECOND"
            | "TIMEZONE_HOUR"
            | "TIMEZONE_MINUTE"
            | "TIMEZONE_REGION"
            | "TIMEZONE_ABBR"
    )
}

fn is_trim_syntax_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "LEADING" | "TRAILING" | "BOTH" | "FROM") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 {
        return false;
    }

    call_open_index(tokens, token_depths, idx, depth, "TRIM").is_some()
}

fn is_substring_syntax_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "FROM" | "FOR") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    let Some(open_idx) = call_open_index(tokens, token_depths, idx, depth, "SUBSTRING") else {
        return false;
    };

    if word_upper == "FROM" {
        return is_substring_from_keyword(tokens, token_depths, idx, depth, open_idx);
    }

    previous_substring_from_keyword_index(tokens, token_depths, idx, depth, open_idx).is_some()
        && next_substring_argument_token_index(tokens, token_depths, idx, depth).is_some()
}

fn is_substring_from_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    open_idx: usize,
) -> bool {
    previous_substring_argument_token_index(tokens, token_depths, idx, depth, open_idx).is_some()
        && next_substring_argument_token_index(tokens, token_depths, idx, depth).is_some()
}

fn previous_substring_from_keyword_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    open_idx: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > open_idx {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("FROM")
                    && is_substring_from_keyword(
                        tokens,
                        token_depths,
                        scan_idx,
                        depth,
                        open_idx,
                    ) =>
            {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn previous_substring_argument_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    open_idx: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > open_idx {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn next_substring_argument_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => scan_idx += 1,
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == ")" => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn is_from_consuming_function_syntax_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "FROM" | "FOR" | "PLACING") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    let Some((call_name, open_idx)) =
        from_consuming_call_name_and_open_index(tokens, token_depths, idx, depth)
    else {
        return false;
    };

    match word_upper {
        "FROM" => {
            is_from_consuming_function_from_keyword(tokens, token_depths, idx, depth, open_idx)
        }
        "FOR" => {
            matches!(call_name.as_str(), "SUBSTRING" | "OVERLAY")
                && previous_from_consuming_function_from_keyword_index(
                    tokens,
                    token_depths,
                    idx,
                    depth,
                    open_idx,
                )
                .is_some()
                && next_substring_argument_token_index(tokens, token_depths, idx, depth).is_some()
        }
        "PLACING" => {
            call_name == "OVERLAY"
                && previous_substring_argument_token_index(
                    tokens,
                    token_depths,
                    idx,
                    depth,
                    open_idx,
                )
                .is_some()
                && next_substring_argument_token_index(tokens, token_depths, idx, depth).is_some()
        }
        _ => false,
    }
}

fn from_consuming_call_name_and_open_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<(String, usize)> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return None;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let call_idx = scan_idx.checked_sub(1)?;
            let Some(SqlToken::Word(call_name)) = tokens.get(call_idx).copied() else {
                return None;
            };
            let call_name_upper = call_name.to_ascii_uppercase();
            return is_from_consuming_function(call_name_upper.as_str())
                .then_some((call_name_upper, scan_idx));
        }
    }

    None
}

fn is_from_consuming_function_from_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    open_idx: usize,
) -> bool {
    previous_substring_argument_token_index(tokens, token_depths, idx, depth, open_idx).is_some()
        && next_substring_argument_token_index(tokens, token_depths, idx, depth).is_some()
}

fn previous_from_consuming_function_from_keyword_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    open_idx: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > open_idx {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("FROM")
                    && is_from_consuming_function_from_keyword(
                        tokens,
                        token_depths,
                        scan_idx,
                        depth,
                        open_idx,
                    ) =>
            {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn is_datetime_literal_syntax_word(tokens: &[&SqlToken], idx: usize, word_upper: &str) -> bool {
    if matches!(word_upper, "DATE" | "TIME" | "TIMESTAMP" | "INTERVAL") {
        return matches!(tokens.get(idx + 1), Some(SqlToken::String(_)))
            || datetime_literal_string_index_after_intro(tokens, idx).is_some();
    }

    if is_extract_datetime_field_word(word_upper) {
        return datetime_literal_intro_index_before_field(tokens, idx).is_some();
    }

    if word_upper == "TO" {
        return is_datetime_literal_to_keyword(tokens, idx);
    }

    matches!(word_upper, "WITH" | "LOCAL" | "ZONE")
        && datetime_literal_string_index_after_intro(tokens, idx).is_some()
}

fn is_datetime_literal_to_keyword(tokens: &[&SqlToken], idx: usize) -> bool {
    let Some(prev_idx) = idx.checked_sub(1) else {
        return false;
    };
    let Some(SqlToken::Word(prev_word)) = tokens.get(prev_idx).copied() else {
        return false;
    };
    if !is_extract_datetime_field_word(&prev_word.to_ascii_uppercase())
        || datetime_literal_intro_index_before_field(tokens, prev_idx).is_none()
    {
        return false;
    }

    matches!(
        tokens.get(idx.saturating_add(1)).copied(),
        Some(SqlToken::Word(next_word))
            if is_extract_datetime_field_word(&next_word.to_ascii_uppercase())
    )
}

fn datetime_literal_string_index_after_intro(tokens: &[&SqlToken], idx: usize) -> Option<usize> {
    let mut scan_idx = idx.saturating_add(1);
    while scan_idx < tokens.len() {
        match tokens.get(scan_idx).copied() {
            Some(SqlToken::String(_)) => return Some(scan_idx),
            Some(SqlToken::Word(word))
                if matches!(
                    word.to_ascii_uppercase().as_str(),
                    "WITH" | "LOCAL" | "TIME" | "ZONE"
                ) =>
            {
                scan_idx += 1;
            }
            _ => return None,
        }
    }

    None
}

fn datetime_literal_intro_index_before_field(tokens: &[&SqlToken], idx: usize) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        match tokens.get(scan_idx).copied() {
            Some(SqlToken::String(_)) => continue,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TO") => continue,
            Some(SqlToken::Word(word))
                if is_extract_datetime_field_word(&word.to_ascii_uppercase()) =>
            {
                continue
            }
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("INTERVAL") => {
                return Some(scan_idx);
            }
            _ => return None,
        }
    }

    None
}

fn is_json_function_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || json_call_open_index(tokens, token_depths, idx, depth).is_none() {
        return false;
    }

    if matches!(
        word_upper,
        "RETURNING" | "DEFAULT" | "ON" | "ERROR" | "EMPTY" | "PASSING"
    ) {
        return true;
    }

    if is_json_function_option_word(tokens, token_depths, idx, depth, word_upper) {
        return true;
    }

    is_sql_type_spec_word(word_upper)
        && json_returning_keyword_before(tokens, token_depths, idx, depth).is_some()
}

fn is_json_function_option_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    word_upper: &str,
) -> bool {
    match word_upper {
        "FORMAT" => next_word_at_depth_is(tokens, token_depths, idx, depth, "JSON"),
        "JSON" => {
            keyword_before_in_same_call(tokens, token_depths, idx, depth, "FORMAT").is_some()
                || json_returning_keyword_before(tokens, token_depths, idx, depth).is_some()
        }
        "SET" | "INSERT" | "REPLACE" | "REMOVE" | "APPEND" | "RENAME" | "KEEP" => {
            is_json_transform_operation_keyword(tokens, token_depths, idx, depth)
        }
        "IGNORE" | "CREATE" => is_json_transform_option_keyword(tokens, token_depths, idx, depth),
        "MISSING" => is_json_transform_missing_keyword(tokens, token_depths, idx, depth),
        "KEY" => is_json_object_key_keyword(tokens, token_depths, idx, depth),
        "VALUE" => is_json_object_value_keyword(tokens, token_depths, idx, depth),
        "ABSENT" => is_json_absent_on_null_keyword(tokens, token_depths, idx, depth),
        "WITH" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "WRAPPER")
                || is_json_unique_keys_with_keyword(tokens, token_depths, idx, depth)
        }
        "WITHOUT" => next_word_at_depth_is(tokens, token_depths, idx, depth, "WRAPPER"),
        "WRAPPER" => {
            keyword_before_in_same_call(tokens, token_depths, idx, depth, "WITH").is_some()
                || keyword_before_in_same_call(tokens, token_depths, idx, depth, "WITHOUT")
                    .is_some()
        }
        "KEYS" => is_json_unique_keys_keyword(tokens, token_depths, idx, depth),
        "STRICT" => is_json_strict_option_keyword(tokens, token_depths, idx, depth),
        "PRETTY" | "ASCII" | "TRUNCATE" => {
            json_returning_keyword_before(tokens, token_depths, idx, depth).is_some()
        }
        "TRUE" | "FALSE" | "UNKNOWN" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "ON")
                && next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
                    |on_idx| next_word_at_depth_is(tokens, token_depths, on_idx, depth, "ERROR"),
                )
        }
        _ => false,
    }
}

fn is_json_object_key_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    if !is_json_object_function_call(tokens, token_depths, idx, depth) {
        return false;
    }

    let Some(next_idx) = next_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if matches!(
        tokens.get(next_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("VALUE")
    ) {
        return false;
    }

    json_object_value_keyword_after(tokens, token_depths, next_idx, depth).is_some()
}

fn is_json_transform_operation_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(open_idx) = json_call_open_index(tokens, token_depths, idx, depth) else {
        return false;
    };
    let Some(call_idx) = open_idx.checked_sub(1) else {
        return false;
    };
    if !matches!(
        tokens.get(call_idx).copied(),
        Some(SqlToken::Word(call_name)) if call_name.eq_ignore_ascii_case("JSON_TRANSFORM")
    ) {
        return false;
    }

    previous_json_transform_operation_separator(tokens, token_depths, idx, depth).is_some()
        && next_json_transform_path_string_index(tokens, token_depths, idx, depth).is_some()
}

fn previous_json_transform_operation_separator(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," => return Some(scan_idx),
            _ => return None,
        }
    }

    None
}

fn next_json_transform_path_string_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => scan_idx += 1,
            Some(SqlToken::String(_)) => return Some(scan_idx),
            _ => return None,
        }
    }

    None
}

fn is_json_transform_option_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    next_word_at_depth_is(tokens, token_depths, idx, depth, "ON")
        && next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|on_idx| {
            next_word_at_depth_is(tokens, token_depths, on_idx, depth, "MISSING")
        })
        && previous_json_transform_operation_keyword(tokens, token_depths, idx, depth).is_some()
}

fn is_json_transform_missing_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(on_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(on_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON")
    ) {
        return false;
    }

    let Some(option_idx) = previous_word_index_at_depth(tokens, token_depths, on_idx, depth) else {
        return false;
    };
    matches!(
        tokens.get(option_idx).copied(),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("IGNORE") || word.eq_ignore_ascii_case("CREATE")
    ) && is_json_transform_option_keyword(tokens, token_depths, option_idx, depth)
}

fn previous_json_transform_operation_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Word(word))
                if matches!(
                    word.to_ascii_uppercase().as_str(),
                    "SET" | "INSERT" | "REPLACE" | "REMOVE" | "APPEND" | "RENAME" | "KEEP"
                ) && is_json_transform_operation_keyword(
                    tokens,
                    token_depths,
                    scan_idx,
                    depth,
                ) =>
            {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn is_json_object_value_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    is_json_object_function_call(tokens, token_depths, idx, depth)
        && previous_json_object_key_token_index(tokens, token_depths, idx, depth).is_some()
        && next_member_of_collection_token_index(tokens, token_depths, idx, depth).is_some()
}

fn is_json_absent_on_null_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(on_idx) = next_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(on_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON")
    ) {
        return false;
    }

    next_word_at_depth_is(tokens, token_depths, on_idx, depth, "NULL")
}

fn is_json_unique_keys_with_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(unique_idx) = next_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(unique_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("UNIQUE")
    ) {
        return false;
    }

    next_word_at_depth_is(tokens, token_depths, unique_idx, depth, "KEYS")
}

fn is_json_unique_keys_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(unique_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(unique_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("UNIQUE")
    ) {
        return false;
    }

    previous_word_index_at_depth(tokens, token_depths, unique_idx, depth).is_some_and(|with_idx| {
        is_json_unique_keys_with_keyword(tokens, token_depths, with_idx, depth)
    })
}

fn is_json_strict_option_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
        match tokens.get(prev_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("KEYS") => {
                is_json_unique_keys_keyword(tokens, token_depths, prev_idx, depth)
            }
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NULL") => {
                previous_json_null_option_keyword(tokens, token_depths, prev_idx, depth).is_some()
            }
            _ => false,
        }
    })
}

fn previous_json_null_option_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    null_idx: usize,
    depth: usize,
) -> Option<usize> {
    let on_idx = previous_word_index_at_depth(tokens, token_depths, null_idx, depth)?;
    if !matches!(
        tokens.get(on_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON")
    ) {
        return None;
    }

    let option_idx = previous_word_index_at_depth(tokens, token_depths, on_idx, depth)?;
    matches!(
        tokens.get(option_idx).copied(),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("ABSENT") || word.eq_ignore_ascii_case("NULL")
    )
    .then_some(option_idx)
}

fn is_json_object_function_call(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(open_idx) = json_call_open_index(tokens, token_depths, idx, depth) else {
        return false;
    };
    let Some(call_idx) = open_idx.checked_sub(1) else {
        return false;
    };

    matches!(
        tokens.get(call_idx).copied(),
        Some(SqlToken::Word(call_name))
            if matches!(
                call_name.to_ascii_uppercase().as_str(),
                "JSON_OBJECT" | "JSON_OBJECTAGG"
            )
    )
}

fn json_object_value_keyword_after(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    start_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = start_idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => scan_idx += 1,
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == ")" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("VALUE") => {
                return next_member_of_collection_token_index(
                    tokens,
                    token_depths,
                    scan_idx,
                    depth,
                )
                .map(|_| scan_idx);
            }
            _ => scan_idx += 1,
        }
    }

    None
}

fn previous_json_object_key_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    value_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = value_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("KEY") => return None,
            Some(SqlToken::Word(_)) | Some(SqlToken::String(_)) => return Some(scan_idx),
            Some(SqlToken::Symbol(sym)) if sym == ")" => return Some(scan_idx),
            Some(_) => return None,
            None => return None,
        }
    }

    None
}

fn is_listagg_overflow_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "ON" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "OVERFLOW")
                && listagg_overflow_action_after(tokens, token_depths, idx, depth).is_some()
        }
        "OVERFLOW" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|on_idx| {
                matches!(
                    tokens.get(on_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON")
                ) && listagg_overflow_action_after(tokens, token_depths, on_idx, depth).is_some()
            })
        }
        "TRUNCATE" | "ERROR" => previous_listagg_overflow_index(tokens, token_depths, idx, depth)
            .is_some_and(|overflow_idx| {
                listagg_overflow_action_after_overflow(tokens, token_depths, overflow_idx, depth)
                    == Some(idx)
            }),
        "WITH" | "WITHOUT" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "COUNT")
                && previous_listagg_overflow_index(tokens, token_depths, idx, depth).is_some()
        }
        "COUNT" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word))
                        if word.eq_ignore_ascii_case("WITH")
                            || word.eq_ignore_ascii_case("WITHOUT")
                ) && previous_listagg_overflow_index(tokens, token_depths, prev_idx, depth)
                    .is_some()
            })
        }
        _ => false,
    }
}

fn listagg_overflow_action_after(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let overflow_idx = next_word_index_at_depth(tokens, token_depths, idx, depth)?;
    if !matches!(
        tokens.get(overflow_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OVERFLOW")
    ) {
        return None;
    }

    listagg_overflow_action_after_overflow(tokens, token_depths, overflow_idx, depth)
}

fn listagg_overflow_action_after_overflow(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    overflow_idx: usize,
    depth: usize,
) -> Option<usize> {
    let action_idx = next_word_index_at_depth(tokens, token_depths, overflow_idx, depth)?;
    matches!(
        tokens.get(action_idx).copied(),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("TRUNCATE") || word.eq_ignore_ascii_case("ERROR")
    )
    .then_some(action_idx)
}

fn previous_listagg_overflow_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OVERFLOW") => {
                let on_idx = previous_word_index_at_depth(tokens, token_depths, scan_idx, depth)?;
                return matches!(
                    tokens.get(on_idx).copied(),
                    Some(SqlToken::Word(on_word)) if on_word.eq_ignore_ascii_case("ON")
                )
                .then_some(scan_idx);
            }
            _ => {}
        }
    }

    None
}

fn is_xmlparse_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || call_open_index(tokens, token_depths, idx, depth, "XMLPARSE").is_none() {
        return false;
    }

    match word_upper {
        "CONTENT" | "DOCUMENT" => {
            xmlparse_intro_keyword_index(tokens, token_depths, idx, depth) == Some(idx)
        }
        "WELLFORMED" => {
            previous_xmlparse_intro_keyword_index(tokens, token_depths, idx, depth).is_some()
        }
        _ => false,
    }
}

fn xmlparse_intro_keyword_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let open_idx = call_open_index(tokens, token_depths, idx, depth, "XMLPARSE")?;
    next_word_index_at_depth(tokens, token_depths, open_idx, depth)
}

fn previous_xmlparse_intro_keyword_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let open_idx = call_open_index(tokens, token_depths, idx, depth, "XMLPARSE")?;
    let intro_idx = next_word_index_at_depth(tokens, token_depths, open_idx, depth)?;
    if intro_idx >= idx {
        return None;
    }

    matches!(
        tokens.get(intro_idx).copied(),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("CONTENT") || word.eq_ignore_ascii_case("DOCUMENT")
    )
    .then_some(intro_idx)
}

fn is_xmlroot_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || call_open_index(tokens, token_depths, idx, depth, "XMLROOT").is_none() {
        return false;
    }

    match word_upper {
        "VERSION" | "STANDALONE" => {
            xmlroot_has_argument_separator_before(tokens, token_depths, idx, depth)
        }
        "YES" => previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
            |standalone_idx| {
                matches!(
                    tokens.get(standalone_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("STANDALONE")
                ) && is_xmlroot_syntax_word(tokens, token_depths, standalone_idx, "STANDALONE")
            },
        ),
        "NO" => {
            let Some(prev_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
            else {
                return false;
            };
            matches!(
                tokens.get(prev_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("STANDALONE")
            ) && is_xmlroot_syntax_word(tokens, token_depths, prev_idx, "STANDALONE")
        }
        "VALUE" => {
            let Some(no_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
            else {
                return false;
            };
            if !matches!(
                tokens.get(no_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NO")
            ) {
                return false;
            }
            let Some(standalone_idx) =
                previous_word_index_at_depth(tokens, token_depths, no_idx, depth)
            else {
                return false;
            };
            matches!(
                tokens.get(standalone_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("STANDALONE")
            ) && is_xmlroot_syntax_word(tokens, token_depths, standalone_idx, "STANDALONE")
        }
        _ => false,
    }
}

fn xmlroot_has_argument_separator_before(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return false;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," => return true,
            _ => return false,
        }
    }

    false
}

fn is_xmlserialize_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || call_open_index(tokens, token_depths, idx, depth, "XMLSERIALIZE").is_none() {
        return false;
    }

    match word_upper {
        "CONTENT" | "DOCUMENT" => {
            xmlserialize_intro_keyword_index(tokens, token_depths, idx, depth) == Some(idx)
        }
        "NO" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "INDENT")
                && xmlserialize_type_spec_before(tokens, token_depths, idx, depth).is_some()
        }
        "INDENT" => {
            xmlserialize_type_spec_before(tokens, token_depths, idx, depth).is_some()
                && (previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
                    |prev_idx| {
                        matches!(
                            tokens.get(prev_idx).copied(),
                            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NO")
                        )
                    },
                ) || next_word_at_depth_is(tokens, token_depths, idx, depth, "SIZE"))
        }
        "SIZE" => previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
            |indent_idx| {
                matches!(
                    tokens.get(indent_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("INDENT")
                ) && xmlserialize_type_spec_before(tokens, token_depths, indent_idx, depth)
                    .is_some()
            },
        ),
        _ => false,
    }
}

fn xmlserialize_intro_keyword_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let open_idx = call_open_index(tokens, token_depths, idx, depth, "XMLSERIALIZE")?;
    next_word_index_at_depth(tokens, token_depths, open_idx, depth)
}

fn xmlserialize_type_spec_before(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if is_sql_type_spec_word(&word.to_ascii_uppercase()) => {
                let as_idx = previous_word_index_at_depth(tokens, token_depths, scan_idx, depth)?;
                return matches!(
                    tokens.get(as_idx).copied(),
                    Some(SqlToken::Word(as_word)) if as_word.eq_ignore_ascii_case("AS")
                )
                .then_some(scan_idx);
            }
            _ => {}
        }
    }

    None
}

fn is_xml_generation_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    if call_open_index(tokens, token_depths, idx, depth, "XMLELEMENT").is_some() {
        match word_upper {
            "NAME" => return is_xmlelement_name_keyword(tokens, token_depths, idx, depth),
            "EVALNAME" => return is_xmlelement_evalname_keyword(tokens, token_depths, idx, depth),
            "ENTITYESCAPING" | "NONENTITYESCAPING" => {
                return is_xml_generation_escaping_keyword(tokens, token_depths, idx, depth);
            }
            _ => {
                if is_xmlelement_name_identifier(tokens, token_depths, idx, depth) {
                    return true;
                }
            }
        }
    }

    if call_open_index(tokens, token_depths, idx, depth, "XMLPI").is_some() {
        match word_upper {
            "NAME" => return is_xmlpi_name_keyword(tokens, token_depths, idx, depth),
            _ => {
                if is_xmlpi_name_identifier(tokens, token_depths, idx, depth) {
                    return true;
                }
            }
        }
    }

    if call_open_index(tokens, token_depths, idx, depth, "XMLFOREST").is_some()
        || call_open_index(tokens, token_depths, idx, depth, "XMLATTRIBUTES").is_some()
    {
        if matches!(word_upper, "ENTITYESCAPING" | "NONENTITYESCAPING")
            && is_xml_generation_escaping_keyword(tokens, token_depths, idx, depth)
        {
            return true;
        }

        return is_xml_generation_alias_after_as(tokens, token_depths, idx, depth);
    }

    false
}

fn is_xmlelement_name_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(open_idx) = call_open_index(tokens, token_depths, idx, depth, "XMLELEMENT") else {
        return false;
    };
    if next_word_index_at_depth(tokens, token_depths, open_idx, depth) != Some(idx) {
        let Some(prev_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth) else {
            return false;
        };
        if !is_xml_generation_escaping_keyword(tokens, token_depths, prev_idx, depth) {
            return false;
        }
    }

    next_word_index_at_depth(tokens, token_depths, idx, depth).is_some()
}

fn is_xmlelement_evalname_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(open_idx) = call_open_index(tokens, token_depths, idx, depth, "XMLELEMENT") else {
        return false;
    };

    next_word_index_at_depth(tokens, token_depths, open_idx, depth) == Some(idx)
        && next_member_of_collection_token_index(tokens, token_depths, idx, depth).is_some()
}

fn is_xml_generation_escaping_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    if matches!(
        next_word_index_at_depth(tokens, token_depths, idx, depth).and_then(|next_idx| {
            tokens.get(next_idx).copied()
        }),
        Some(SqlToken::Word(word))
            if word.eq_ignore_ascii_case("NAME") || word.eq_ignore_ascii_case("EVALNAME")
    ) {
        return true;
    }

    let Some(next_idx) = next_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };

    xml_generation_alias_after_expression(tokens, token_depths, next_idx, depth).is_some()
}

fn is_xmlelement_name_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|name_idx| {
        matches!(
            tokens.get(name_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NAME")
        ) && is_xmlelement_name_keyword(tokens, token_depths, name_idx, depth)
    })
}

fn is_xmlpi_name_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    let Some(open_idx) = call_open_index(tokens, token_depths, idx, depth, "XMLPI") else {
        return false;
    };
    if next_word_index_at_depth(tokens, token_depths, open_idx, depth) != Some(idx) {
        return false;
    }

    next_word_index_at_depth(tokens, token_depths, idx, depth).is_some()
}

fn is_xmlpi_name_identifier(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|name_idx| {
        matches!(
            tokens.get(name_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NAME")
        ) && is_xmlpi_name_keyword(tokens, token_depths, name_idx, depth)
    })
}

fn is_xml_generation_alias_after_as(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> bool {
    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|as_idx| {
        matches!(
            tokens.get(as_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS")
        ) && previous_xml_generation_alias_expression_token_index(
            tokens,
            token_depths,
            as_idx,
            depth,
        )
        .is_some()
    })
}

fn previous_xml_generation_alias_expression_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    as_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = as_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS") => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn xml_generation_alias_after_expression(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    expr_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = expr_idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => scan_idx += 1,
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == ")" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AS") => {
                return next_word_index_at_depth(tokens, token_depths, scan_idx, depth);
            }
            _ => scan_idx += 1,
        }
    }

    None
}

fn next_word_at_depth_is(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    keyword: &str,
) -> bool {
    next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|next_idx| {
        matches!(
            tokens.get(next_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case(keyword)
        )
    })
}

fn next_word_index_at_depth(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(_)) => return Some(scan_idx),
            Some(SqlToken::Comment(_)) => {
                scan_idx += 1;
            }
            _ => return None,
        }
    }

    None
}

fn previous_word_index_at_depth(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(_)) => return Some(scan_idx),
            Some(SqlToken::Comment(_)) => {}
            _ => return None,
        }
    }

    None
}

fn json_call_open_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return None;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let call_idx = scan_idx.checked_sub(1)?;
            return matches!(
                tokens.get(call_idx).copied(),
                Some(SqlToken::Word(call_name)) if is_json_function_name(call_name)
            )
            .then_some(scan_idx);
        }
    }

    None
}

fn json_returning_keyword_before(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("RETURNING") => {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn is_json_function_name(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "JSON_VALUE"
            | "JSON_EXISTS"
            | "JSON_QUERY"
            | "JSON_SERIALIZE"
            | "JSON_OBJECT"
            | "JSON_ARRAY"
            | "JSON_OBJECTAGG"
            | "JSON_ARRAYAGG"
            | "JSON_TRANSFORM"
    )
}

fn is_json_predicate_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    if word_upper != "JSON" {
        return is_json_predicate_option_word(tokens, token_depths, idx, depth, word_upper);
    }

    let Some(is_idx) = json_predicate_is_index(tokens, token_depths, idx, depth) else {
        return false;
    };

    previous_json_predicate_subject_token_index(tokens, token_depths, is_idx, depth).is_some()
}

fn json_predicate_is_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    json_idx: usize,
    depth: usize,
) -> Option<usize> {
    let prev_idx = previous_word_index_at_depth(tokens, token_depths, json_idx, depth)?;
    match tokens.get(prev_idx).copied() {
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IS") => Some(prev_idx),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NOT") => {
            previous_word_index_at_depth(tokens, token_depths, prev_idx, depth).and_then(|is_idx| {
                matches!(
                    tokens.get(is_idx).copied(),
                    Some(SqlToken::Word(is_word)) if is_word.eq_ignore_ascii_case("IS")
                )
                .then_some(is_idx)
            })
        }
        _ => None,
    }
}

fn is_json_predicate_option_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    word_upper: &str,
) -> bool {
    if !matches!(
        word_upper,
        "VALUE"
            | "ARRAY"
            | "OBJECT"
            | "SCALAR"
            | "STRICT"
            | "LAX"
            | "WITH"
            | "WITHOUT"
            | "UNIQUE"
            | "KEYS"
    ) {
        return false;
    }

    let Some(json_idx) = previous_json_predicate_json_index(tokens, token_depths, idx, depth)
    else {
        return false;
    };
    let Some(is_idx) = json_predicate_is_index(tokens, token_depths, json_idx, depth) else {
        return false;
    };
    if previous_json_predicate_subject_token_index(tokens, token_depths, is_idx, depth).is_none() {
        return false;
    }

    match word_upper {
        "VALUE" | "ARRAY" | "OBJECT" | "SCALAR" | "STRICT" | "LAX" => true,
        "WITH" | "WITHOUT" => next_word_at_depth_is(tokens, token_depths, idx, depth, "UNIQUE"),
        "UNIQUE" => {
            let prev_is_with_or_without =
                previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
                    |prev_idx| {
                        matches!(
                            tokens.get(prev_idx).copied(),
                            Some(SqlToken::Word(word))
                                if word.eq_ignore_ascii_case("WITH")
                                    || word.eq_ignore_ascii_case("WITHOUT")
                        )
                    },
                );
            prev_is_with_or_without
                && next_word_at_depth_is(tokens, token_depths, idx, depth, "KEYS")
        }
        "KEYS" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("UNIQUE")
                )
            })
        }
        _ => false,
    }
}

fn previous_json_predicate_json_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("JSON") => {
                return Some(scan_idx);
            }
            Some(SqlToken::Word(word))
                if matches!(
                    word.to_ascii_uppercase().as_str(),
                    "VALUE"
                        | "ARRAY"
                        | "OBJECT"
                        | "SCALAR"
                        | "STRICT"
                        | "LAX"
                        | "WITH"
                        | "WITHOUT"
                        | "UNIQUE"
                        | "KEYS"
                ) => {}
            Some(SqlToken::Comment(_)) => {}
            _ => return None,
        }
    }

    None
}

fn previous_json_predicate_subject_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    is_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = is_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IS") => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn is_is_of_type_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "OF" => is_of_type_of_keyword(tokens, token_depths, idx, depth),
        "TYPE" => is_of_type_type_keyword(tokens, token_depths, idx, depth),
        "ONLY" => is_inside_is_of_type_list(tokens, token_depths, idx).is_some(),
        _ => {
            if is_inside_is_of_type_list(tokens, token_depths, idx).is_some() {
                return true;
            }

            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                match tokens.get(prev_idx).copied() {
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TYPE") => {
                        is_of_type_type_keyword(tokens, token_depths, prev_idx, depth)
                    }
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OF") => {
                        is_of_type_of_keyword(tokens, token_depths, prev_idx, depth)
                    }
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ONLY") => {
                        previous_word_index_at_depth(tokens, token_depths, prev_idx, depth)
                            .is_some_and(|marker_idx| match tokens.get(marker_idx).copied() {
                                Some(SqlToken::Word(marker))
                                    if marker.eq_ignore_ascii_case("TYPE") =>
                                {
                                    is_of_type_type_keyword(tokens, token_depths, marker_idx, depth)
                                }
                                Some(SqlToken::Word(marker))
                                    if marker.eq_ignore_ascii_case("OF") =>
                                {
                                    is_of_type_of_keyword(tokens, token_depths, marker_idx, depth)
                                }
                                _ => false,
                            })
                    }
                    _ => false,
                }
            })
        }
    }
}

fn is_of_type_type_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    type_idx: usize,
    depth: usize,
) -> bool {
    previous_word_index_at_depth(tokens, token_depths, type_idx, depth).is_some_and(|of_idx| {
        matches!(
            tokens.get(of_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OF")
        ) && is_of_type_of_keyword(tokens, token_depths, of_idx, depth)
    })
}

fn is_of_type_of_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    of_idx: usize,
    depth: usize,
) -> bool {
    let Some(prev_idx) = previous_word_index_at_depth(tokens, token_depths, of_idx, depth) else {
        return false;
    };

    let is_idx = match tokens.get(prev_idx).copied() {
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IS") => prev_idx,
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NOT") => {
            let Some(is_idx) = previous_word_index_at_depth(tokens, token_depths, prev_idx, depth)
            else {
                return false;
            };
            if !matches!(
                tokens.get(is_idx).copied(),
                Some(SqlToken::Word(is_word)) if is_word.eq_ignore_ascii_case("IS")
            ) {
                return false;
            }
            is_idx
        }
        _ => return false,
    };

    previous_is_of_type_subject_token_index(tokens, token_depths, is_idx, depth).is_some()
}

fn is_inside_is_of_type_list(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
) -> Option<usize> {
    let depth = token_depths.get(idx).copied()?;
    if depth == 0 {
        return None;
    }

    let list_depth = depth.saturating_sub(1);
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < list_depth {
            return None;
        }
        if scan_depth != list_depth {
            continue;
        }

        if !matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            continue;
        }

        let marker_idx = previous_word_index_at_depth(tokens, token_depths, scan_idx, list_depth)?;
        return match tokens.get(marker_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TYPE") => {
                is_of_type_type_keyword(tokens, token_depths, marker_idx, list_depth)
                    .then_some(marker_idx)
            }
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("OF") => {
                is_of_type_of_keyword(tokens, token_depths, marker_idx, list_depth)
                    .then_some(marker_idx)
            }
            _ => None,
        };
    }

    None
}

fn previous_is_of_type_subject_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    is_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = is_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IS") => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn is_member_of_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "MEMBER" => is_member_of_member_keyword(tokens, token_depths, idx, depth),
        "OF" => previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
            |member_idx| {
                matches!(
                    tokens.get(member_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("MEMBER")
                ) && is_member_of_member_keyword(tokens, token_depths, member_idx, depth)
                    && next_member_of_collection_token_index(tokens, token_depths, idx, depth)
                        .is_some()
            },
        ),
        _ => false,
    }
}

fn is_member_of_member_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    member_idx: usize,
    depth: usize,
) -> bool {
    if !next_word_at_depth_is(tokens, token_depths, member_idx, depth, "OF") {
        return false;
    }

    let subject_before_idx = previous_word_index_at_depth(tokens, token_depths, member_idx, depth)
        .and_then(|prev_idx| {
            matches!(
                tokens.get(prev_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NOT")
            )
            .then_some(prev_idx)
        });
    let subject_boundary_idx = subject_before_idx.unwrap_or(member_idx);

    previous_member_of_subject_token_index(tokens, token_depths, subject_boundary_idx, depth)
        .is_some()
}

fn previous_member_of_subject_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("MEMBER") => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn next_member_of_collection_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    of_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = of_idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => scan_idx += 1,
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == ")" => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn is_submultiset_of_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "SUBMULTISET" => is_submultiset_of_keyword(tokens, token_depths, idx, depth),
        "OF" => previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
            |submultiset_idx| {
                matches!(
                    tokens.get(submultiset_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("SUBMULTISET")
                ) && is_submultiset_of_keyword(tokens, token_depths, submultiset_idx, depth)
                    && next_member_of_collection_token_index(tokens, token_depths, idx, depth)
                        .is_some()
            },
        ),
        _ => false,
    }
}

fn is_submultiset_of_keyword(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    submultiset_idx: usize,
    depth: usize,
) -> bool {
    next_word_at_depth_is(tokens, token_depths, submultiset_idx, depth, "OF")
        && previous_member_of_subject_token_index(tokens, token_depths, submultiset_idx, depth)
            .is_some()
}

fn is_multiset_operator_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "MULTISET" => is_multiset_operator_marker(tokens, token_depths, idx, depth),
        "UNION" | "EXCEPT" | "INTERSECT" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(
                |multiset_idx| {
                    matches!(
                        tokens.get(multiset_idx).copied(),
                        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("MULTISET")
                    ) && is_multiset_operator_marker(tokens, token_depths, multiset_idx, depth)
                        && next_member_of_collection_token_index(tokens, token_depths, idx, depth)
                            .is_some()
                },
            )
        }
        _ => false,
    }
}

fn is_multiset_operator_marker(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    multiset_idx: usize,
    depth: usize,
) -> bool {
    let Some(op_idx) = next_word_index_at_depth(tokens, token_depths, multiset_idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(op_idx).copied(),
        Some(SqlToken::Word(word))
            if matches!(
                word.to_ascii_uppercase().as_str(),
                "UNION" | "EXCEPT" | "INTERSECT"
            )
    ) {
        return false;
    }

    previous_member_of_subject_token_index(tokens, token_depths, multiset_idx, depth).is_some()
}

fn is_collection_condition_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "EMPTY" => collection_condition_is_index(tokens, token_depths, idx, depth).is_some(),
        "A" => {
            next_word_at_depth_is(tokens, token_depths, idx, depth, "SET")
                && collection_condition_is_index(tokens, token_depths, idx, depth).is_some()
        }
        "SET" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|a_idx| {
                matches!(
                    tokens.get(a_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("A")
                ) && collection_condition_is_index(tokens, token_depths, a_idx, depth).is_some()
            })
        }
        _ => false,
    }
}

fn collection_condition_is_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let prev_idx = previous_word_index_at_depth(tokens, token_depths, idx, depth)?;
    let is_idx = match tokens.get(prev_idx).copied() {
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("IS") => prev_idx,
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NOT") => {
            let is_idx = previous_word_index_at_depth(tokens, token_depths, prev_idx, depth)?;
            matches!(
                tokens.get(is_idx).copied(),
                Some(SqlToken::Word(is_word)) if is_word.eq_ignore_ascii_case("IS")
            )
            .then_some(is_idx)?
        }
        _ => return None,
    };

    previous_is_of_type_subject_token_index(tokens, token_depths, is_idx, depth).map(|_| is_idx)
}

fn is_like_escape_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if word_upper != "ESCAPE" {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    like_operator_before(tokens, token_depths, idx, depth).is_some()
        && next_member_of_collection_token_index(tokens, token_depths, idx, depth).is_some()
}

fn like_operator_before(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if is_like_operator_word(word) => return Some(scan_idx),
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn is_like_operator_word(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "LIKE" | "LIKEC" | "LIKE2" | "LIKE4"
    )
}

fn is_sounds_like_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if word_upper != "SOUNDS" {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    next_word_at_depth_is(tokens, token_depths, idx, depth, "LIKE")
        && previous_sounds_like_subject_token_index(tokens, token_depths, idx, depth).is_some()
}

fn previous_sounds_like_subject_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    sounds_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = sounds_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Symbol(sym)) if sym == ")" => return Some(scan_idx),
            Some(SqlToken::Word(word)) if is_expression_keyword(&word.to_ascii_uppercase()) => {
                return None;
            }
            Some(SqlToken::Word(_)) | Some(SqlToken::String(_)) => return Some(scan_idx),
            Some(_) => return None,
            None => return None,
        }
    }

    None
}

fn is_numeric_condition_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "NAN" | "INFINITE") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    collection_condition_is_index(tokens, token_depths, idx, depth).is_some()
}

fn is_boolean_condition_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "TRUE" | "FALSE" | "UNKNOWN") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    collection_condition_is_index(tokens, token_depths, idx, depth).is_some()
}

fn is_distinct_from_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if word_upper != "FROM" {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    let Some(distinct_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(
        tokens.get(distinct_idx).copied(),
        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("DISTINCT")
    ) {
        return false;
    }

    let Some(is_idx) = collection_condition_is_index(tokens, token_depths, distinct_idx, depth)
    else {
        return false;
    };

    previous_is_of_type_subject_token_index(tokens, token_depths, is_idx, depth).is_some()
        && next_member_of_collection_token_index(tokens, token_depths, idx, depth).is_some()
}

fn is_xmlquery_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    let is_xmlquery = call_open_index(tokens, token_depths, idx, depth, "XMLQUERY").is_some();
    let is_xmlexists = call_open_index(tokens, token_depths, idx, depth, "XMLEXISTS").is_some();
    if depth == 0 || (!is_xmlquery && !is_xmlexists) {
        return false;
    }

    if word_upper == "PASSING" {
        return true;
    }

    if is_xmlexists {
        return word_upper == "VALUE"
            && keyword_before_in_same_call(tokens, token_depths, idx, depth, "PASSING").is_some();
    }

    if word_upper == "RETURNING" {
        return true;
    }

    word_upper == "CONTENT"
        && keyword_before_in_same_call(tokens, token_depths, idx, depth, "RETURNING").is_some()
}

fn keyword_before_in_same_call(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
    keyword: &str,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case(keyword) => {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn is_window_frame_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || call_open_index(tokens, token_depths, idx, depth, "OVER").is_none() {
        return false;
    }

    match word_upper {
        "GROUPS" => next_word_at_depth_is(tokens, token_depths, idx, depth, "BETWEEN"),
        "EXCLUDE" => {
            next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|next_idx| {
                matches!(
                    tokens.get(next_idx).copied(),
                    Some(SqlToken::Word(word))
                        if matches!(
                            word.to_ascii_uppercase().as_str(),
                            "CURRENT" | "GROUP" | "TIES" | "NO"
                        )
                )
            })
        }
        "TIES" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("EXCLUDE")
                )
            })
        }
        "NO" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("EXCLUDE")
                )
            }) && next_word_at_depth_is(tokens, token_depths, idx, depth, "OTHERS")
        }
        "OTHERS" => {
            let Some(no_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
            else {
                return false;
            };
            if !matches!(
                tokens.get(no_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NO")
            ) {
                return false;
            }
            previous_word_index_at_depth(tokens, token_depths, no_idx, depth).is_some_and(
                |exclude_idx| {
                    matches!(
                        tokens.get(exclude_idx).copied(),
                        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("EXCLUDE")
                    )
                },
            )
        }
        _ => false,
    }
}

fn is_analytic_null_treatment_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if !matches!(word_upper, "IGNORE" | "RESPECT") {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|next_idx| {
        matches!(
            tokens.get(next_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NULLS")
        )
    })
}

fn is_nth_value_from_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    if word_upper != "FROM" {
        return false;
    }

    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if !next_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|next_idx| {
        matches!(
            tokens.get(next_idx).copied(),
            Some(SqlToken::Word(word))
                if matches!(word.to_ascii_uppercase().as_str(), "FIRST" | "LAST")
        )
    }) {
        return false;
    }

    let Some(close_idx) = previous_token_index_at_depth(tokens, token_depths, idx, depth) else {
        return false;
    };
    if !matches!(tokens.get(close_idx), Some(SqlToken::Symbol(sym)) if sym == ")") {
        return false;
    }

    closed_call_name_before(tokens, token_depths, close_idx, depth)
        .is_some_and(|call_name| call_name.eq_ignore_ascii_case("NTH_VALUE"))
}

fn previous_token_index_at_depth(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth == depth && !matches!(tokens.get(scan_idx), Some(SqlToken::Comment(_))) {
            return Some(scan_idx);
        }
    }

    None
}

fn closed_call_name_before<'a>(
    tokens: &[&'a SqlToken],
    token_depths: &[usize],
    close_idx: usize,
    depth: usize,
) -> Option<&'a str> {
    let mut nested = 0usize;
    let mut scan_idx = close_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Symbol(sym)) if sym == ")" && scan_depth > depth => {
                nested = nested.saturating_add(1);
            }
            Some(SqlToken::Symbol(sym)) if sym == "(" && scan_depth == depth => {
                if nested > 0 {
                    nested -= 1;
                    continue;
                }
                let call_idx = scan_idx.checked_sub(1)?;
                return match tokens.get(call_idx).copied() {
                    Some(SqlToken::Word(call_name)) => Some(call_name),
                    _ => None,
                };
            }
            Some(SqlToken::Symbol(sym)) if sym == "(" && nested > 0 => {
                nested -= 1;
            }
            _ => {}
        }
    }

    None
}

fn is_at_time_zone_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    match word_upper {
        "AT" => {
            let Some(next_idx) = next_word_index_at_depth(tokens, token_depths, idx, depth) else {
                return false;
            };
            match tokens.get(next_idx).copied() {
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("LOCAL") => true,
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TIME") => {
                    next_word_at_depth_is(tokens, token_depths, next_idx, depth, "ZONE")
                }
                _ => false,
            }
        }
        "TIME" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AT")
                )
            }) && next_word_at_depth_is(tokens, token_depths, idx, depth, "ZONE")
        }
        "ZONE" => {
            let Some(prev_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
            else {
                return false;
            };
            if !matches!(
                tokens.get(prev_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TIME")
            ) {
                return false;
            }
            previous_word_index_at_depth(tokens, token_depths, prev_idx, depth).is_some_and(
                |at_idx| {
                    matches!(
                        tokens.get(at_idx).copied(),
                        Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AT")
                    )
                },
            )
        }
        "LOCAL" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("AT")
                )
            })
        }
        _ => false,
    }
}

fn is_collate_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };

    if word_upper == "COLLATE" {
        return collate_collation_name_index(tokens, token_depths, idx, depth).is_some()
            && previous_collate_expression_token_index(tokens, token_depths, idx, depth).is_some();
    }

    previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
        matches!(
            tokens.get(prev_idx).copied(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("COLLATE")
        ) && collate_collation_name_index(tokens, token_depths, prev_idx, depth) == Some(idx)
            && previous_collate_expression_token_index(tokens, token_depths, prev_idx, depth)
                .is_some()
    })
}

fn collate_collation_name_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    collate_idx: usize,
    depth: usize,
) -> Option<usize> {
    next_word_index_at_depth(tokens, token_depths, collate_idx, depth).filter(|next_idx| {
        matches!(
            tokens.get(*next_idx).copied(),
            Some(SqlToken::Word(word)) if is_identifier_word_token(word)
        )
    })
}

fn previous_collate_expression_token_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    collate_idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = collate_idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }

        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Comment(_)) => {}
            Some(SqlToken::Symbol(sym)) if sym == "," || sym == "(" => return None,
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("COLLATE") => return None,
            Some(_) => return Some(scan_idx),
            None => return None,
        }
    }

    None
}

fn is_conversion_default_syntax_word(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    word_upper: &str,
) -> bool {
    let Some(depth) = token_depths.get(idx).copied() else {
        return false;
    };
    if depth == 0 || conversion_call_open_index(tokens, token_depths, idx, depth).is_none() {
        return false;
    }

    match word_upper {
        "DEFAULT" => next_word_after_conversion_default(tokens, token_depths, idx, depth).is_some(),
        "ON" => {
            previous_conversion_default_index(tokens, token_depths, idx, depth).is_some()
                && next_word_at_depth_is(tokens, token_depths, idx, depth, "CONVERSION")
        }
        "CONVERSION" => {
            previous_word_index_at_depth(tokens, token_depths, idx, depth).is_some_and(|prev_idx| {
                matches!(
                    tokens.get(prev_idx).copied(),
                    Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON")
                )
            }) && next_word_at_depth_is(tokens, token_depths, idx, depth, "ERROR")
                && previous_conversion_default_index(tokens, token_depths, idx, depth).is_some()
        }
        "ERROR" => {
            let Some(prev_idx) = previous_word_index_at_depth(tokens, token_depths, idx, depth)
            else {
                return false;
            };
            if !matches!(
                tokens.get(prev_idx).copied(),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("CONVERSION")
            ) {
                return false;
            }
            previous_conversion_default_index(tokens, token_depths, prev_idx, depth).is_some()
        }
        _ => false,
    }
}

fn conversion_call_open_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth.saturating_sub(1) {
            return None;
        }
        if scan_depth == depth.saturating_sub(1)
            && matches!(tokens.get(scan_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            let call_idx = scan_idx.checked_sub(1)?;
            return matches!(
                tokens.get(call_idx).copied(),
                Some(SqlToken::Word(call_name)) if is_conversion_function_name(call_name)
            )
            .then_some(scan_idx);
        }
    }

    None
}

fn previous_conversion_default_index(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx;
    while scan_idx > 0 {
        scan_idx -= 1;
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            continue;
        }
        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("DEFAULT") => {
                return Some(scan_idx);
            }
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            _ => {}
        }
    }

    None
}

fn next_word_after_conversion_default(
    tokens: &[&SqlToken],
    token_depths: &[usize],
    idx: usize,
    depth: usize,
) -> Option<usize> {
    let mut scan_idx = idx.saturating_add(1);
    while scan_idx < tokens.len() {
        let scan_depth = token_depths.get(scan_idx).copied().unwrap_or(0);
        if scan_depth < depth {
            return None;
        }
        if scan_depth != depth {
            scan_idx += 1;
            continue;
        }
        match tokens.get(scan_idx).copied() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON") => return None,
            Some(SqlToken::Word(_)) | Some(SqlToken::String(_)) => return Some(scan_idx),
            Some(SqlToken::Symbol(sym)) if sym == "," => return None,
            Some(SqlToken::Comment(_)) => {
                scan_idx += 1;
            }
            _ => return None,
        }
    }

    None
}

fn is_conversion_function_name(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "TO_NUMBER"
            | "TO_DATE"
            | "TO_TIMESTAMP"
            | "TO_TIMESTAMP_TZ"
            | "TO_BINARY_FLOAT"
            | "TO_BINARY_DOUBLE"
    )
}

fn is_sql_type_spec_word(word: &str) -> bool {
    matches!(
        word,
        "NUMBER"
            | "NUMERIC"
            | "DECIMAL"
            | "DEC"
            | "INTEGER"
            | "INT"
            | "SMALLINT"
            | "FLOAT"
            | "DOUBLE"
            | "PRECISION"
            | "REAL"
            | "BINARY_FLOAT"
            | "BINARY_DOUBLE"
            | "CHAR"
            | "CHARACTER"
            | "NATIONAL"
            | "VARCHAR"
            | "VARCHAR2"
            | "NCHAR"
            | "NVARCHAR2"
            | "RAW"
            | "ROWID"
            | "UROWID"
            | "DATE"
            | "TIMESTAMP"
            | "TIME"
            | "INTERVAL"
            | "YEAR"
            | "TO"
            | "MONTH"
            | "DAY"
            | "HOUR"
            | "MINUTE"
            | "SECOND"
            | "WITH"
            | "WITHOUT"
            | "LOCAL"
            | "ZONE"
            | "CLOB"
            | "NCLOB"
            | "BLOB"
            | "BOOLEAN"
            | "SIGNED"
            | "UNSIGNED"
    )
}

fn collect_table_function_columns(
    tokens: &[SqlToken],
    columns: &mut Vec<String>,
    seen: &mut HashSet<String>,
    allow_unparenthesized_marker: bool,
) {
    let mut idx = 0usize;
    while idx < tokens.len() {
        let Some(SqlToken::Word(word)) = tokens.get(idx) else {
            idx += 1;
            continue;
        };
        let marker_upper = word.to_ascii_uppercase();
        if marker_upper != "COLUMNS" && marker_upper != "WITH" {
            idx += 1;
            continue;
        }

        if marker_upper == "COLUMNS"
            && matches!(prev_non_comment_token(tokens, idx), Some((SqlToken::Symbol(sym), _)) if sym == ",")
        {
            idx += 1;
            continue;
        }

        let next_idx = next_non_comment_index(tokens, idx.saturating_add(1));
        if marker_upper == "WITH"
            && !matches!(tokens.get(next_idx), Some(SqlToken::Symbol(sym)) if sym == "(")
        {
            idx += 1;
            continue;
        }
        if matches!(tokens.get(next_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
            if let Some((range, after_paren)) = extract_parenthesized_range(tokens, next_idx) {
                let range_tokens = token_range_slice(tokens, range);
                append_table_function_column_items(range_tokens, columns, seen);
                // Recurse to capture nested `... COLUMNS (...)` clauses.
                collect_table_function_columns(range_tokens, columns, seen, false);
                idx = after_paren;
                continue;
            }
            idx += 1;
            continue;
        }

        if !allow_unparenthesized_marker {
            idx += 1;
            continue;
        }

        if next_idx < tokens.len() {
            let tail = &tokens[next_idx..];
            append_table_function_column_items(tail, columns, seen);
            collect_table_function_columns(tail, columns, seen, false);
        }
        break;
    }
}

fn append_table_function_column_items(
    tokens: &[SqlToken],
    columns: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for item_tokens in split_top_level_symbol_groups(tokens, ",") {
        if let Some(column) = resolve_table_function_column_name(&item_tokens) {
            let key = column_lookup_key(&column);
            if seen.insert(key) {
                columns.push(column);
            }
        }
    }
}

fn extract_incomplete_qualified_item_prefix(item_tokens: &[&SqlToken]) -> Option<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|t| !matches!(t, SqlToken::Comment(_)))
        .collect();
    if meaningful.len() < 2 {
        return None;
    }

    let qualifier = match meaningful.first().copied() {
        Some(SqlToken::Word(word)) if is_identifier_word_token(word) => {
            strip_identifier_quotes(word)
        }
        _ => return None,
    };
    match meaningful.get(1) {
        Some(SqlToken::Symbol(dot)) if dot == "." => {}
        _ => return None,
    }

    // `qualifier.column` is a complete reference and should not be treated as
    // an incomplete prefix for fallback inference.
    if let Some(SqlToken::Word(word)) = meaningful.get(2).copied() {
        if is_identifier_word_token(word) {
            return None;
        }
    }

    Some(qualifier)
}

fn is_expression_keyword(word: &str) -> bool {
    matches!(
        word,
        "AS" | "DISTINCT"
            | "ALL"
            | "UNIQUE"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "NULL"
            | "WHERE"
            | "AND"
            | "OR"
            | "NOT"
            | "IN"
            | "IS"
            | "LIKE"
            | "LIKEC"
            | "LIKE2"
            | "LIKE4"
            | "BETWEEN"
            | "OVER"
            | "PARTITION"
            | "ORDER"
            | "BY"
            | "KEEP"
            | "DENSE_RANK"
            | "FIRST"
            | "LAST"
            | "WITHIN"
            | "GROUP"
            | "ASC"
            | "DESC"
            | "NULLS"
            | "ROWS"
            | "RANGE"
            | "CURRENT"
            | "ROW"
            | "UNBOUNDED"
            | "PRECEDING"
            | "FOLLOWING"
    )
}

fn is_match_recognize_pattern_keyword(word: &str) -> bool {
    matches!(word, "PERMUTE" | "SUBSET")
}

fn parse_identifier_segment(tokens: &[SqlToken]) -> Vec<String> {
    let mut columns = Vec::new();
    let mut meaningful_start = None;
    for (idx, token) in tokens.iter().enumerate() {
        if !matches!(token, SqlToken::Comment(_)) {
            meaningful_start = Some(idx);
            break;
        }
    }
    let Some(start_idx) = meaningful_start else {
        return columns;
    };

    if matches!(tokens.get(start_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        if let Some((range, _)) = extract_parenthesized_range(tokens, start_idx) {
            columns = parse_identifier_words_top_level(token_range_slice(tokens, range));
            dedup_columns_case_insensitive(&mut columns);
            return columns;
        }
    }

    if let Some(name) = parse_first_identifier_word(tokens) {
        columns.push(name);
    }
    columns
}

fn parse_first_identifier_word(tokens: &[SqlToken]) -> Option<String> {
    for token in tokens {
        if let SqlToken::Word(word) = token {
            if is_identifier_word_token(word) {
                return Some(strip_identifier_quotes(word));
            }
        }
    }
    None
}

fn parse_identifier_words_top_level(tokens: &[SqlToken]) -> Vec<String> {
    let token_depths = paren_depths(tokens);
    let mut columns = Vec::new();

    for (idx, token) in tokens.iter().enumerate() {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            if !is_identifier_word_token(word) {
                continue;
            }
            let upper = word.to_ascii_uppercase();
            if upper == "AS" {
                continue;
            }
            columns.push(strip_identifier_quotes(word));
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_pivot_generated_columns_from_in_segment(
    tokens: &[SqlToken],
    aggregate_aliases: &[String],
) -> Vec<String> {
    let open_idx = next_non_comment_index(tokens, 0);
    let Some(SqlToken::Symbol(sym)) = tokens.get(open_idx) else {
        return Vec::new();
    };
    if sym != "(" {
        return Vec::new();
    }

    let Some((range, _)) = extract_parenthesized_range(tokens, open_idx) else {
        return Vec::new();
    };
    let in_list_tokens = token_range_slice(tokens, range);
    let mut columns = Vec::new();

    for item_tokens in split_top_level_symbol_groups(in_list_tokens, ",") {
        if let Some(column) = parse_pivot_in_item_output_column(&item_tokens) {
            if aggregate_aliases.is_empty() {
                columns.push(column);
            } else {
                columns.extend(
                    aggregate_aliases
                        .iter()
                        .map(|alias| combine_pivot_generated_column(&column, alias)),
                );
            }
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn combine_pivot_generated_column(pivot_column: &str, aggregate_alias: &str) -> String {
    let pivot_trimmed = pivot_column.trim();
    let aggregate_trimmed = aggregate_alias.trim();
    if is_quoted_identifier(pivot_trimmed) || is_quoted_identifier(aggregate_trimmed) {
        let pivot = strip_identifier_quotes(pivot_trimmed);
        let aggregate = strip_identifier_quotes(aggregate_trimmed);
        let escaped = format!("{}_{}", pivot, aggregate).replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        format!("{pivot_trimmed}_{aggregate_trimmed}")
    }
}

fn parse_pivot_in_item_output_column(item_tokens: &[&SqlToken]) -> Option<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect();
    if meaningful.is_empty() {
        return None;
    }

    let mut idx = 0usize;
    while idx < meaningful.len() {
        if let SqlToken::Word(word) = meaningful[idx] {
            if word.eq_ignore_ascii_case("AS") {
                let mut alias_idx = idx.saturating_add(1);
                while alias_idx < meaningful.len() {
                    if let SqlToken::Word(alias) = meaningful[alias_idx] {
                        if is_identifier_word_token(alias) {
                            return Some(output_identifier_suggestion(alias));
                        }
                    }
                    if !matches!(meaningful[alias_idx], SqlToken::Comment(_)) {
                        break;
                    }
                    alias_idx += 1;
                }
                break;
            }
        }
        idx += 1;
    }

    if meaningful.len() == 1 {
        match meaningful.first().copied() {
            Some(SqlToken::String(value)) => {
                return parse_pivot_in_string_literal_output(value);
            }
            Some(SqlToken::Word(word)) if is_numeric_literal_word(word) => {
                return Some(word.to_string());
            }
            _ => {}
        }
    }

    if let Some(SqlToken::Word(last_word)) = meaningful.last().copied() {
        if is_identifier_word_token(last_word) {
            return Some(output_identifier_suggestion(last_word));
        }
    }

    if let Some(SqlToken::Word(first_word)) = meaningful.first().copied() {
        if is_identifier_word_token(first_word) {
            return Some(output_identifier_suggestion(first_word));
        }
    }

    None
}

fn parse_pivot_in_string_literal_output(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        return None;
    }
    if !trimmed.starts_with('\'') || !trimmed.ends_with('\'') {
        return None;
    }

    let inner = &trimmed[1..trimmed.len() - 1];
    Some(inner.replace("''", "'"))
}

fn is_numeric_literal_word(word: &str) -> bool {
    let mut has_digit = false;
    for ch in word.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
            continue;
        }

        if matches!(ch, '+' | '-' | '.' | '_' | 'e' | 'E') {
            continue;
        }

        return false;
    }

    has_digit
}

fn parse_unpivot_output_segment(tokens: &[SqlToken]) -> Vec<String> {
    let start_idx = next_non_comment_index(tokens, 0);
    if start_idx >= tokens.len() {
        return Vec::new();
    }

    if matches!(tokens.get(start_idx), Some(SqlToken::Symbol(sym)) if sym == "(") {
        if let Some((range, _)) = extract_parenthesized_range(tokens, start_idx) {
            let mut columns =
                parse_identifier_suggestions_top_level(token_range_slice(tokens, range));
            dedup_columns_case_insensitive(&mut columns);
            return columns;
        }
    }

    if let Some(column) = parse_first_identifier_suggestion(&tokens[start_idx..]) {
        return vec![column];
    }

    Vec::new()
}

fn parse_first_identifier_suggestion(tokens: &[SqlToken]) -> Option<String> {
    for token in tokens {
        if let SqlToken::Word(word) = token {
            if is_identifier_word_token(word) {
                return Some(output_identifier_suggestion(word));
            }
        }
    }
    None
}

fn parse_identifier_suggestions_top_level(tokens: &[SqlToken]) -> Vec<String> {
    let token_depths = paren_depths(tokens);
    let mut columns = Vec::new();

    for (idx, token) in tokens.iter().enumerate() {
        if !is_top_level_depth(&token_depths, idx) {
            continue;
        }
        if let SqlToken::Word(word) = token {
            if !is_identifier_word_token(word) {
                continue;
            }
            let upper = word.to_ascii_uppercase();
            if upper == "AS" {
                continue;
            }
            columns.push(output_identifier_suggestion(word));
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_unpivot_source_columns_from_in_segment(tokens: &[SqlToken]) -> Vec<String> {
    let open_idx = next_non_comment_index(tokens, 0);
    let Some(SqlToken::Symbol(sym)) = tokens.get(open_idx) else {
        return Vec::new();
    };
    if sym != "(" {
        return Vec::new();
    }

    let Some((range, _)) = extract_parenthesized_range(tokens, open_idx) else {
        return Vec::new();
    };
    let list_tokens = token_range_slice(tokens, range);
    let mut columns = Vec::new();

    for item_tokens in split_top_level_symbol_groups(list_tokens, ",") {
        columns.extend(parse_unpivot_in_item_source_columns(&item_tokens));
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_unpivot_in_item_source_columns(item_tokens: &[&SqlToken]) -> Vec<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect();
    if meaningful.is_empty() {
        return Vec::new();
    }

    let starts_with_tuple =
        matches!(meaningful.first().copied(), Some(SqlToken::Symbol(sym)) if sym == "(");
    let target_depth = if starts_with_tuple { 1usize } else { 0usize };
    let mut depth = 0usize;
    let mut columns = Vec::new();

    for token in meaningful {
        match token {
            SqlToken::Symbol(sym) if sym == "(" => {
                depth = depth.saturating_add(1);
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                depth = depth.saturating_sub(1);
            }
            SqlToken::Word(word) => {
                if depth == 0 && word.eq_ignore_ascii_case("AS") {
                    break;
                }
                if depth == target_depth && is_identifier_word_token(word) {
                    columns.push(strip_identifier_quotes(word));
                }
            }
            _ => {}
        }
    }

    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_model_measure_columns(tokens: &[SqlToken]) -> Vec<String> {
    let mut columns = Vec::new();
    for item_tokens in split_top_level_symbol_groups(tokens, ",") {
        if let Some(column) = parse_model_measure_output_column(&item_tokens) {
            columns.push(column);
        }
    }
    dedup_columns_case_insensitive(&mut columns);
    columns
}

fn parse_model_measure_output_column(item_tokens: &[&SqlToken]) -> Option<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect();
    if meaningful.is_empty() {
        return None;
    }

    let mut depth = 0usize;
    let mut idx = 0usize;
    while idx < meaningful.len() {
        match meaningful[idx] {
            SqlToken::Symbol(sym) if sym == "(" => {
                depth = depth.saturating_add(1);
            }
            SqlToken::Symbol(sym) if sym == ")" => {
                depth = depth.saturating_sub(1);
            }
            SqlToken::Word(word) if depth == 0 && word.eq_ignore_ascii_case("AS") => {
                if let Some(SqlToken::Word(alias)) = meaningful.get(idx.saturating_add(1)).copied()
                {
                    if is_identifier_word_token(alias) {
                        return Some(output_identifier_suggestion(alias));
                    }
                }
                return None;
            }
            _ => {}
        }
        idx += 1;
    }

    if meaningful.len() >= 2 {
        let alias_idx = meaningful.len().saturating_sub(1);
        if let Some(SqlToken::Word(alias)) = meaningful.get(alias_idx).copied() {
            let upper = alias.to_ascii_uppercase();
            if is_identifier_word_token(alias)
                && !sql_text::is_oracle_sql_keyword(&upper)
                && !is_match_recognize_clause_boundary_keyword(&upper)
            {
                let previous = meaningful[alias_idx.saturating_sub(1)];
                if !matches!(previous, SqlToken::Symbol(sym) if sym == ".") {
                    return Some(output_identifier_suggestion(alias));
                }
            }
        }
    }

    parse_simple_identifier_path_output_column(&meaningful)
}

fn parse_simple_identifier_path_output_column(tokens: &[&SqlToken]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }

    let mut parser_state = IdentifierPathState::ExpectIdentifier;
    let mut last_identifier = None;
    for token in tokens {
        match token {
            SqlToken::Word(word)
                if parser_state.expects_identifier() && is_identifier_word_token(word) =>
            {
                last_identifier = Some(output_identifier_suggestion(word));
                parser_state = IdentifierPathState::ExpectDot;
            }
            SqlToken::Symbol(sym) if parser_state.expects_dot() && sym == "." => {
                parser_state = IdentifierPathState::ExpectIdentifier;
            }
            _ => return None,
        }
    }

    if parser_state.expects_identifier() {
        return None;
    }
    last_identifier
}

fn output_identifier_suggestion(word: &str) -> String {
    let trimmed = word.trim();
    if is_quoted_identifier(trimmed) {
        trimmed.to_string()
    } else {
        strip_identifier_quotes(trimmed)
    }
}

fn next_non_comment_index(tokens: &[SqlToken], start: usize) -> usize {
    let mut idx = start.min(tokens.len());
    while idx < tokens.len() {
        if !matches!(tokens[idx], SqlToken::Comment(_)) {
            break;
        }
        idx += 1;
    }
    idx
}

fn dedup_columns_case_insensitive(columns: &mut Vec<String>) {
    let mut seen = HashSet::new();
    columns.retain(|column| seen.insert(column_lookup_key(column)));
}

fn remove_columns_case_insensitive(columns: &mut Vec<String>, remove: &[String]) {
    if columns.is_empty() || remove.is_empty() {
        return;
    }
    let remove_set: HashSet<String> = remove.iter().map(|name| column_lookup_key(name)).collect();
    columns.retain(|column| !remove_set.contains(&column_lookup_key(column)));
}

fn column_lookup_key(column: &str) -> String {
    let trimmed = column.trim();
    if is_quoted_identifier(trimmed) {
        strip_identifier_quotes(trimmed).to_ascii_uppercase()
    } else {
        column.to_ascii_uppercase()
    }
}

fn resolve_table_function_column_name(item_tokens: &[&SqlToken]) -> Option<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|token| !matches!(token, SqlToken::Comment(_)))
        .collect();
    if let Some(column) = parse_leading_bracket_identifier(&meaningful) {
        return Some(column);
    }

    let first_word = meaningful.iter().copied().find_map(|token| match token {
        SqlToken::Comment(_) => None,
        SqlToken::Word(word) => Some(word.as_str()),
        _ => None,
    })?;

    let upper = first_word.to_ascii_uppercase();
    if is_table_function_item_leading_keyword(&upper)
        && table_function_leading_keyword_is_syntax(&upper, &meaningful)
    {
        return None;
    }
    if !is_identifier_word_token(first_word) {
        return None;
    }

    Some(output_identifier_suggestion(first_word))
}

fn parse_leading_bracket_identifier(tokens: &[&SqlToken]) -> Option<String> {
    if !matches!(tokens.first(), Some(SqlToken::Symbol(sym)) if sym == "[") {
        return None;
    }

    let mut inner = String::new();
    let mut idx = 1usize;
    while idx < tokens.len() {
        match tokens[idx] {
            SqlToken::Symbol(sym) if sym == "]" => {
                if matches!(tokens.get(idx + 1), Some(SqlToken::Symbol(next)) if next == "]") {
                    push_bracket_identifier_part(&mut inner, "]", false);
                    idx += 2;
                    continue;
                }
                if inner.is_empty() {
                    return None;
                }
                return Some(format!("[{}]", inner.replace("]", "]]")));
            }
            SqlToken::Word(word) | SqlToken::String(word) => {
                push_bracket_identifier_part(&mut inner, word, true);
            }
            SqlToken::Symbol(symbol) => {
                push_bracket_identifier_part(&mut inner, symbol, false);
            }
            SqlToken::Comment(_) => {}
        }
        idx += 1;
    }

    None
}

fn table_function_leading_keyword_is_syntax(first_upper: &str, item_tokens: &[&SqlToken]) -> bool {
    let Some(next_token) = item_tokens
        .iter()
        .copied()
        .skip(1)
        .find(|token| !matches!(token, SqlToken::Comment(_)))
    else {
        return matches!(first_upper, "FOR" | "NESTED" | "ORDINALITY");
    };

    let SqlToken::Word(next_word) = next_token else {
        return matches!(first_upper, "FOR" | "NESTED" | "ORDINALITY");
    };
    let next_upper = next_word.to_ascii_uppercase();
    match first_upper {
        "NESTED" => matches!(next_upper.as_str(), "PATH" | "COLUMNS"),
        "FOR" => next_upper == "ORDINALITY",
        "ORDINALITY" => true,
        "ON" => is_table_function_item_leading_keyword(&next_upper),
        "ERROR" | "NULL" | "DEFAULT" => next_upper == "ON",
        "KEEP" | "OMIT" => next_upper == "QUOTES",
        "CONDITIONAL" | "UNCONDITIONAL" | "WITH" | "WITHOUT" => next_upper == "WRAPPER",
        _ => false,
    }
}

fn is_table_function_item_leading_keyword(word: &str) -> bool {
    sql_text::is_table_function_item_leading_keyword(word)
}

fn extract_select_list_tokens(tokens: &[SqlToken]) -> &[SqlToken] {
    let start = select_list_start_index(tokens);
    let end = select_list_end_index(tokens, start);
    &tokens[start..end]
}

fn select_list_start_index(tokens: &[SqlToken]) -> usize {
    let mut idx = 0usize;
    let token_depths = paren_depths(tokens);

    // Find the statement-level SELECT keyword. Ignore nested SELECTs inside
    // CTE bodies, subqueries, and scalar expressions.
    while idx < tokens.len() {
        if !is_top_level_depth(&token_depths, idx) {
            idx += 1;
            continue;
        }
        match &tokens[idx] {
            SqlToken::Word(w) if w.eq_ignore_ascii_case("SELECT") => {
                idx += 1;
                break;
            }
            SqlToken::Comment(_) => {
                idx += 1;
            }
            _ => {
                idx += 1;
            }
        }
    }

    // Skip DISTINCT / ALL / UNIQUE.
    while idx < tokens.len() {
        match &tokens[idx] {
            SqlToken::Word(w) => {
                let upper = w.to_ascii_uppercase();
                if matches!(upper.as_str(), "DISTINCT" | "ALL" | "UNIQUE") {
                    idx += 1;
                } else {
                    break;
                }
            }
            SqlToken::Comment(_) => {
                idx += 1;
            }
            _ => break,
        }
    }

    idx
}

fn select_list_end_index(tokens: &[SqlToken], start: usize) -> usize {
    let token_depths = paren_depths(tokens);
    let mut idx = start;

    while idx < tokens.len() {
        let token = &tokens[idx];
        if is_top_level_depth(&token_depths, idx) {
            if let SqlToken::Word(w) = token {
                let upper = w.to_ascii_uppercase();
                if matches!(upper.as_str(), "FROM" | "INTO" | "BULK") {
                    break;
                }
            }
        }

        idx += 1;
    }

    idx
}

// Only reached from `extract_select_list_wildcard_tables`, which is itself
// test-only for now.
#[allow(dead_code)]
fn append_wildcard_item_tables(
    item_tokens: &[&SqlToken],
    tables_in_scope: &[ScopedTableRef],
    tables: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|t| !matches!(t, SqlToken::Comment(_)))
        .collect();

    if meaningful.is_empty() {
        return;
    }

    // Unqualified wildcard: `*`
    if meaningful.len() == 1 {
        if let SqlToken::Symbol(s) = meaningful[0] {
            if s == "*" {
                for table in resolve_all_scope_tables(tables_in_scope) {
                    let key = table.to_ascii_uppercase();
                    if seen.insert(key) {
                        tables.push(table);
                    }
                }
                return;
            }
        }
    }

    // Qualified wildcard: `alias.*` or dotted qualifiers like `schema.table.*`.
    if meaningful.len() >= 3 {
        let last = meaningful[meaningful.len() - 1];
        let dot = meaningful[meaningful.len() - 2];
        if let (SqlToken::Symbol(star), SqlToken::Symbol(dot_sym)) = (last, dot) {
            if star == "*" && dot_sym == "." {
                if let Some(normalized) =
                    normalize_dotted_identifier_tokens(&meaningful[..meaningful.len() - 2])
                {
                    for table in resolve_qualifier_tables(&normalized, tables_in_scope) {
                        let key = table.to_ascii_uppercase();
                        if seen.insert(key) {
                            tables.push(table);
                        }
                    }
                }
            }
        }
    }
}

fn append_wildcard_item_scopes(
    item_tokens: &[&SqlToken],
    tables_in_scope: &[ScopedTableRef],
    tables: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|t| !matches!(t, SqlToken::Comment(_)))
        .collect();

    if meaningful.is_empty() {
        return;
    }

    if meaningful.len() == 1 {
        if let SqlToken::Symbol(s) = meaningful[0] {
            if s == "*" {
                for table in resolve_all_scope_wildcard_scopes(tables_in_scope) {
                    let key = normalize_identifier_for_scope_dedupe(&table);
                    if seen.insert(key) {
                        tables.push(table);
                    }
                }
                return;
            }
        }
    }

    if meaningful.len() >= 3 {
        let last = meaningful[meaningful.len() - 1];
        let dot = meaningful[meaningful.len() - 2];
        if let (SqlToken::Symbol(star), SqlToken::Symbol(dot_sym)) = (last, dot) {
            if star == "*" && dot_sym == "." {
                if let Some(normalized) =
                    normalize_dotted_identifier_tokens(&meaningful[..meaningful.len() - 2])
                {
                    let resolved = tables_in_scope
                        .iter()
                        .find_map(|table_ref| {
                            table_ref
                                .alias
                                .as_deref()
                                .filter(|alias| alias.eq_ignore_ascii_case(&normalized))
                                .map(str::to_string)
                        })
                        .map(|alias| vec![alias])
                        .unwrap_or_else(|| resolve_qualifier_tables(&normalized, tables_in_scope));
                    for table in resolved {
                        let key = normalize_identifier_for_scope_dedupe(&table);
                        if seen.insert(key) {
                            tables.push(table);
                        }
                    }
                }
            }
        }
    }
}

fn resolve_all_scope_wildcard_scopes(tables_in_scope: &[ScopedTableRef]) -> Vec<String> {
    let mut ordered_tables: Vec<(usize, &ScopedTableRef)> =
        tables_in_scope.iter().enumerate().collect();
    ordered_tables.sort_by(|(left_idx, left), (right_idx, right)| {
        right
            .depth
            .cmp(&left.depth)
            .then_with(|| left_idx.cmp(right_idx))
    });

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for (_, table_ref) in ordered_tables {
        let scope_name = table_ref.alias.as_ref().unwrap_or(&table_ref.name);
        let upper = normalize_identifier_for_scope_dedupe(scope_name);
        if seen.insert(upper) {
            result.push(scope_name.clone());
        }
    }

    result
}

fn normalize_dotted_identifier_tokens(tokens: &[&SqlToken]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let mut parser_state = IdentifierPathState::ExpectIdentifier;
    for token in tokens {
        if parser_state.expects_identifier() {
            if let SqlToken::Word(word) = token {
                let segment = strip_identifier_quotes(word);
                if segment.is_empty() {
                    return None;
                }
                parts.push(segment);
                parser_state = IdentifierPathState::ExpectDot;
            } else {
                return None;
            }
        } else if let SqlToken::Symbol(sym) = token {
            if sym == "." {
                parser_state = IdentifierPathState::ExpectIdentifier;
            } else {
                return None;
            }
        } else {
            return None;
        }
    }

    if parser_state.expects_identifier() || parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PivotMode {
    Regular,
    Xml,
}

impl PivotMode {
    fn should_skip_generated_columns(self) -> bool {
        matches!(self, Self::Xml)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentifierPathState {
    ExpectIdentifier,
    ExpectDot,
}

impl IdentifierPathState {
    fn expects_identifier(self) -> bool {
        matches!(self, Self::ExpectIdentifier)
    }

    fn expects_dot(self) -> bool {
        matches!(self, Self::ExpectDot)
    }
}

/// Given the tokens of a single SELECT item, determine the output column name.
fn resolve_item_column_name(item_tokens: &[&SqlToken]) -> Option<String> {
    let meaningful: Vec<&SqlToken> = item_tokens
        .iter()
        .copied()
        .filter(|t| !matches!(t, SqlToken::Comment(_)))
        .collect();

    if meaningful.is_empty() {
        return None;
    }

    // Check for lone `*`
    if meaningful.len() == 1 {
        if let SqlToken::Symbol(s) = meaningful[0] {
            if s == "*" {
                return None;
            }
        }
    }

    // Check for `qualifier.*` pattern
    if meaningful.len() >= 2 {
        if let SqlToken::Symbol(s) = meaningful[meaningful.len() - 1] {
            if s == "*" {
                return None;
            }
        }
    }

    let last = meaningful.last()?;
    let second_last = if meaningful.len() >= 2 {
        Some(meaningful[meaningful.len() - 2])
    } else {
        None
    };

    // Case 1: Explicit alias `... AS alias_name`
    if let SqlToken::Word(alias) = last {
        if !is_identifier_word_token(alias) {
            return None;
        }
        if let Some(SqlToken::Word(kw)) = second_last {
            if kw.eq_ignore_ascii_case("AS") {
                return Some(output_identifier_suggestion(alias));
            }
        }
    }

    // Case 2: Implicit alias — last token is a Word following `)` or another Word
    if let SqlToken::Word(alias) = last {
        let alias_upper = alias.to_ascii_uppercase();
        if !is_select_item_trailing_keyword(&alias_upper) {
            if let Some(prev) = second_last {
                let is_implicit = match prev {
                    SqlToken::Symbol(s) if s == ")" => true,
                    SqlToken::Word(_) => {
                        // Two consecutive words: the second is an implicit alias
                        // unless the first is AS (already handled above)
                        meaningful.len() > 1
                    }
                    SqlToken::String(_) => true,
                    SqlToken::Symbol(s) if s == "." => false, // qualifier.column, not alias
                    _ => false,
                };
                if is_implicit {
                    return Some(output_identifier_suggestion(alias));
                }
            }
        }
    }

    // Case 3: Simple column reference (single word)
    if meaningful.len() == 1 {
        if let SqlToken::Word(name) = meaningful[0] {
            if !is_identifier_word_token(name) {
                return None;
            }
            return Some(output_identifier_suggestion(name));
        }
    }

    // Case 4: Qualified column path `a.b` or deeper paths like `a.b.c`.
    if let Some(column) = parse_simple_identifier_path_output_column(&meaningful) {
        return Some(column);
    }

    // Expression without alias — cannot determine column name
    None
}

fn is_select_item_trailing_keyword(word: &str) -> bool {
    matches!(
        word,
        "FROM"
            | "WHERE"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "INTO"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "MINUS"
            | "FETCH"
            | "FOR"
            | "LIMIT"
            | "OFFSET"
            | "CONNECT"
            | "START"
            | "BULK"
    )
}

#[cfg(test)]
mod wildcard_resolution_tests {
    use super::*;
    use crate::ui::sql_editor::SqlEditorWidget;

    #[test]
    fn dotted_qualified_wildcard_prefers_full_table_name_over_alias_match() {
        let sql =
            "SELECT schema_a.emp.* FROM schema_a.emp e JOIN dept emp ON e.deptno = emp.deptno";
        let tokens = SqlEditorWidget::tokenize_sql(sql);
        let ctx = analyze_cursor_context(&tokens, tokens.len());

        let wildcard_tables = extract_select_list_wildcard_tables(&tokens, &ctx.tables_in_scope);

        assert_eq!(wildcard_tables, vec!["schema_a.emp".to_string()]);
    }
}

#[cfg(test)]
mod tests;
