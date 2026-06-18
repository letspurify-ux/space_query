#[derive(Clone)]
struct AsyncIntellisenseParseResult {
    analysis: IntellisenseAnalysis,
    routine_cache: RoutineSymbolCacheEntry,
}

/// Grammatical placement of the unqualified select-list wildcard at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectListWildcardSlot {
    /// A query's own select-list level: both `*` and every `t.*` are offered.
    Full,
    /// Inside an aggregate's argument right after `(`: only the bare `*`
    /// (`COUNT(*)`) is offered.
    CountStarOnly,
    /// Inside a function/expression sub-paren: no wildcard is grammatical.
    None,
}

/// The keyword family that is grammatical where the cursor begins a fresh
/// statement. A top-level SQL statement starts with a DML/DDL/transaction verb;
/// a PL/SQL block position admits the procedural statement keywords plus exactly
/// the construct continuations its enclosing construct allows (resolved by a
/// PL/SQL position scan into a `PlsqlKeywordPolicy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatementStartContext {
    TopLevel,
    Plsql(PlsqlKeywordPolicy),
}

/// Which keyword families are grammatical at a PL/SQL statement/continuation
/// position, resolved from the construct enclosing the cursor and its state.
/// Every flag defaults off; a position scan turns on exactly what the grammar
/// admits there, so the flat keyword dump is filtered to that set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct PlsqlKeywordPolicy {
    /// Procedural statement keywords (`IF`/`LOOP`/`RETURN`/…) and bare
    /// call/assignment targets — a *statement* position (a block body, a loop
    /// body, or a statement `CASE`/`IF` branch), never an expression `CASE` arm.
    allow_statements: bool,
    /// `WHEN` — a `CASE` selector/branch head, or an exception handler.
    allow_when: bool,
    /// `ELSIF` — an `IF` body that has not yet reached its `ELSE`.
    allow_elsif: bool,
    /// `ELSE` — an `IF`/`CASE` body that has not yet reached its `ELSE`.
    allow_else: bool,
    /// `END` — closes the innermost open construct.
    allow_end: bool,
    /// `EXCEPTION` — opens a block's handler section (once, before it has one).
    allow_exception: bool,
    /// `EXIT`/`CONTINUE` — inside (possibly via a nested construct) a loop.
    allow_exit_continue: bool,
}

/// Cursor-position facts that decide which keywords are grammatical at a
/// value/column expression slot, so the base catalog can be filtered down to an
/// allowlist of position-valid keywords instead of dumping the whole catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExpressionKeywordContext {
    /// Cursor sits inside an unclosed `CASE` (its body keywords are valid).
    inside_case: bool,
    /// Cursor is in a query-level `ORDER BY` (admits `ASC`/`DESC`/`NULLS`).
    in_order_by: bool,
    /// Whether the cursor follows a complete operand: `Some(true)` after one
    /// (operators are valid), `Some(false)` where a new operand is expected
    /// (value/expression starters are valid), `None` when ambiguous (both).
    follows_operand: Option<bool>,
    /// Whether the cursor immediately follows a closed call `…)`. The
    /// analytic/aggregate continuations (`OVER`, `KEEP`, `WITHIN GROUP`) are only
    /// grammatical there — `SUM(x) OVER`, `MAX(x) KEEP (...)`,
    /// `LISTAGG(..) WITHIN GROUP (...)` — never after a plain column or literal.
    follows_call: bool,
    /// Whether the cursor immediately follows a closed MySQL full-text
    /// `MATCH(...)` call. `AGAINST` is grammatical only there, not after any
    /// arbitrary function call.
    follows_match_call: bool,
    /// Whether the cursor sits after the first word of MySQL's compound
    /// `SOUNDS LIKE` operator. Only `LIKE` is grammatical there.
    follows_sounds_operator: bool,
    /// Whether MySQL trigger pseudo row `NEW` is valid at this trigger event
    /// (`INSERT`/`UPDATE`).
    mysql_trigger_allows_new: bool,
    /// Whether MySQL trigger pseudo row `OLD` is valid at this trigger event
    /// (`UPDATE`/`DELETE`).
    mysql_trigger_allows_old: bool,
    /// Whether the cursor follows the pattern of an unclosed `LIKE` comparison
    /// (`<expr> LIKE <pattern> |`). `ESCAPE` is grammatical only there —
    /// `name LIKE 'a\_%' ESCAPE '\'` — never after a plain operand.
    follows_like_pattern: bool,
    /// Whether the cursor is inside the top-level argument list of MySQL
    /// `GROUP_CONCAT`, before its optional `SEPARATOR` clause has appeared.
    /// `SEPARATOR` is grammatical only there and only after an expression/order
    /// operand, never after a plain select-list operand.
    inside_group_concat_arguments_before_separator: bool,
    /// Whether the cursor sits at a set-quantifier anchor: right after `SELECT`,
    /// a set operator, or an opening `(`. `DISTINCT`/`UNIQUE`/`DISTINCTROW` are
    /// grammatical only there (`SELECT DISTINCT`, `COUNT(DISTINCT x)`), never as a
    /// general operand (`x = DISTINCT`, `x + DISTINCT`).
    follows_quantifier_anchor: bool,
    /// Whether the cursor is immediately after a comparison operator that can
    /// introduce a quantified comparison (`x = ANY (...)`, `x > ALL (...)`).
    follows_quantified_comparison_operator: bool,
    /// Whether the current query level has a `CONNECT BY` clause. The hierarchical
    /// pseudo-columns/operators (`LEVEL`, `PRIOR`, `CONNECT_BY_ROOT`,
    /// `CONNECT_BY_ISCYCLE`, `CONNECT_BY_ISLEAF`) are grammatical only in a
    /// hierarchical query. (`ROWNUM` is valid in any query, so it is not gated.)
    has_connect_by: bool,
    /// Whether the cursor is in a DML value position (`INSERT … VALUES (…|)`,
    /// `UPDATE … SET col = |`) where the `DEFAULT` value keyword is grammatical.
    in_dml_value_position: bool,
    /// Inferred type of the operand immediately before the cursor. Gates the
    /// type-specific postfix operators: `AT` (datetime), `COLLATE` (character),
    /// `MEMBER`/`SUBMULTISET`/`MULTISET` (collection).
    prev_operand_type: PrecedingOperandType,
    /// Whether the cursor sits where a table/view/synonym name is never a valid
    /// operand: anywhere in a PL/SQL *executable* block body (`RETURN |`,
    /// `RAISE |`, `IF | THEN`, `v := v + |`, `EXCEPTION WHEN |`, a statement
    /// start), right after the assignment `:=` / named-argument `=>` (which also
    /// covers a `DECLARE`-section default), or inside a routine call's argument
    /// list (`dbms_output.put_line(|)`). The relation entries the General-context
    /// base would otherwise dump are dropped as noise (a variable, function,
    /// package or literal still completes). Excludes the PL/SQL declaration type
    /// slot (`v emp%ROWTYPE`) and embedded SQL clauses, which carry their own
    /// phase; in a column context the base has no relations, so this is a no-op.
    in_plsql_value_expression: bool,
    /// Whether the cursor is at a PL/SQL *value-operand* position — inside a
    /// PL/SQL value expression (`in_plsql_value_expression`) *and* directly after
    /// a token that can only introduce an operand, never begin a statement: the
    /// assignment `:=` / named-argument `=>`, a binary/comparison/arithmetic
    /// operator, or a condition/value keyword (`IF`/`ELSIF`/`WHILE`/`RETURN`/
    /// `RAISE`/`AND`/`OR`/`NOT`/`IN`/`LIKE`/`BETWEEN`). This is what lets the
    /// expression-keyword allowlist run in General-context PL/SQL code (dropping
    /// clause/statement keywords like `WHERE`/`WHILE`/`CREATE` that the flat base
    /// dump would otherwise leak into `v := |`) without touching a *statement
    /// start* (`BEGIN |`, `; |`, `THEN |`), where those statement keywords are
    /// the valid completions.
    at_plsql_value_operand: bool,
    /// Whether the cursor names a bind variable — the identifier directly after a
    /// `:` introducer (`WHERE c = :|`, `:b|`). A bind name is a free/session-bind
    /// identifier, never a column/relation/`*`/keyword, so the identifier base and
    /// the wildcard are suppressed there (session bind names still come from the
    /// local-symbol path). The compound `:=` assignment is a distinct token, so it
    /// never trips this.
    at_bind_variable_name: bool,
    /// When the cursor begins a fresh statement, the boundary kind and (for a
    /// PL/SQL block) the enclosing-construct facts that gate the statement-keyword
    /// allowlist. `None` anywhere that is not a statement start, so the flat
    /// keyword dump is filtered only where a statement verb is the sole valid
    /// keyword family.
    statement_start: Option<StatementStartContext>,
}

/// Best-effort type classification of the operand immediately before the cursor,
/// used to gate the operand-type-specific postfix operators. `Unknown` means the
/// type could not be determined; the type-specific operators are then withheld
/// (a never-valid keyword is noise, and an unrecognised operand is treated as
/// not-of-that-type) rather than dumped after every operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrecedingOperandType {
    Datetime,
    Character,
    Collection,
    /// Determined, but none of the type-gated families (e.g. numeric).
    Other,
    Unknown,
}

/// Niladic temporal value keywords/functions that stand as complete operands
/// without a following argument list. Shared by operand-type inference and
/// "did the previous token complete an operand?" classification so the alias
/// slot after `CURRENT_DATE |` behaves the same as after `SYSDATE |`.
const DATETIME_VALUE_WORDS: &[&str] = &[
    "SYSDATE",
    "SYSTIMESTAMP",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "LOCALTIME",
    "LOCALTIMESTAMP",
];

/// SQL keyword constructs whose next token in a value expression must be `(`.
/// Ordinary built-in function names are intentionally excluded: many are valid
/// unquoted column names in practice, so treating all functions this way would
/// hide legitimate alias/operand positions.
const PARENTHESIZED_EXPRESSION_CONSTRUCT_WORDS: &[&str] = &[
    "CAST",
    "CURSOR",
    "EXISTS",
    "EXTRACT",
    "JSON_EXISTS",
    "JSON_QUERY",
    "JSON_VALUE",
    "TREAT",
    "XMLCAST",
    "XMLEXISTS",
    "XMLQUERY",
];

/// Table-source constructs whose next token must be `(` in a `FROM` clause.
/// Kept separate from expression constructs because ordinary table contexts
/// otherwise correctly offer relation names.
const PARENTHESIZED_TABLE_SOURCE_CONSTRUCT_WORDS: &[&str] = &["JSON_TABLE", "TABLE", "XMLTABLE"];

const JSON_ERROR_EMPTY_OPTION_FUNCTION_WORDS: &[&str] =
    &["JSON_EXISTS", "JSON_QUERY", "JSON_TABLE", "JSON_VALUE"];
const JSON_ON_NULL_OPTION_FUNCTION_WORDS: &[&str] =
    &["JSON_ARRAY", "JSON_ARRAYAGG", "JSON_OBJECT", "JSON_OBJECTAGG"];
const JSON_ERROR_EMPTY_TARGET_WORDS: &[&str] = &["ERROR", "EMPTY"];
const JSON_NULL_TARGET_WORDS: &[&str] = &["NULL"];

impl ExpressionKeywordContext {
    /// A non-committal context (used where the expression-keyword filter does not
    /// run, e.g. table/qualified slots) that admits both keyword families.
    #[cfg(test)]
    fn ambiguous() -> Self {
        Self {
            inside_case: false,
            in_order_by: false,
            follows_operand: None,
            follows_call: false,
            follows_match_call: false,
            follows_sounds_operator: false,
            mysql_trigger_allows_new: false,
            mysql_trigger_allows_old: false,
            follows_like_pattern: false,
            inside_group_concat_arguments_before_separator: false,
            follows_quantifier_anchor: false,
            follows_quantified_comparison_operator: false,
            has_connect_by: false,
            in_dml_value_position: false,
            prev_operand_type: PrecedingOperandType::Unknown,
            in_plsql_value_expression: false,
            at_plsql_value_operand: false,
            at_bind_variable_name: false,
            statement_start: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectListWildcardMode {
    None,
    Unqualified,
    Qualified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualifiedCompletionMode {
    RelationColumns,
    RelationMembers,
    ObjectMembers,
}

/// Position kind where a SQL data type is expected. The keyword set differs by
/// position for some dialects (e.g. MySQL `CAST` accepts a restricted grammar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DataTypePosition {
    /// `CAST(expr AS |)` / `TREAT(expr AS |)`.
    Cast,
    /// Column definition type slot in `CREATE TABLE` / `ALTER TABLE`.
    ColumnDef,
    /// PL/SQL type slot: variable/parameter/return/collection-element type.
    Plsql,
}

/// Argument slot inside an `EXTRACT(<field> FROM <source>)` call, before the
/// `FROM`. A datetime field keyword is the only thing valid here, never a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExtractArgPosition {
    /// Right after `EXTRACT(` — the datetime field name slot.
    Field,
    /// After the field name, before `FROM` — only `FROM` follows.
    AwaitingFrom,
}

/// Qualifier slot in an `INTERVAL '<value>' <unit> [TO <unit>]` literal. The
/// value lives in the string, so each slot is keyword-only — never a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntervalUnitSlot {
    /// `INTERVAL '5' |` — the leading qualifier unit.
    Leading,
    /// `INTERVAL '5' DAY |` — only `TO` (or end of literal) follows.
    AwaitingTo,
    /// `INTERVAL '5' DAY TO |` — the trailing qualifier unit.
    Trailing,
}

/// Fixed keyword tail of an `ORDER BY` sort key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderBySortModifierSlot {
    /// `<sort-key> ASC|DESC |` - only `NULLS` can follow before the next key.
    AfterDirection,
    /// `<sort-key> [ASC|DESC] NULLS |` - only `FIRST`/`LAST` can follow.
    AfterNulls,
    /// `<sort-key> [ASC|DESC] NULLS FIRST|LAST |` - the modifier tail is done.
    AfterNullOrdering,
}

/// Fixed keyword tail of an `ORDER BY` item inside a window specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowOrderBySortModifierSlot {
    /// `<sort-key> |` - direction/null-ordering/frame-unit keywords can follow.
    AfterSortKey,
    /// `<sort-key> ASC|DESC |` - null-ordering or a frame unit can follow.
    AfterDirection,
    /// `<sort-key> [ASC|DESC] NULLS |` - only `FIRST`/`LAST`.
    AfterNulls,
    /// `<sort-key> [ASC|DESC] NULLS FIRST|LAST |` - frame units can follow.
    AfterNullOrdering,
}

/// Keyword position within a window frame clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowFrameKeywordSlot {
    /// `ROWS|RANGE|GROUPS |` - a bound expression may also follow.
    AfterUnit,
    /// `... BETWEEN |` - a bound expression may also follow.
    AfterBetween,
    /// `... BETWEEN <bound> AND |` - a bound expression may also follow.
    AfterAnd,
    /// `... <bound-expr> |` - only `PRECEDING`/`FOLLOWING`.
    AfterBoundExpression,
    /// `UNBOUNDED |` - only `PRECEDING`/`FOLLOWING`.
    AfterUnbounded,
    /// `CURRENT |` - only `ROW`.
    AfterCurrent,
    /// `... BETWEEN <first-bound> |` - only `AND`.
    AfterFirstBound,
    /// `<complete-frame-bound> |` - optionally `EXCLUDE`.
    AfterFrameEnd,
    /// `... EXCLUDE |` - only an exclusion kind.
    AfterExclude,
    /// `... EXCLUDE CURRENT |` - only `ROW`.
    AfterExcludeCurrent,
    /// `... EXCLUDE NO |` - only `OTHERS`.
    AfterExcludeNo,
    /// A complete `EXCLUDE ...` tail.
    AfterExcludeEnd,
}

/// Keyword position inside Oracle `KEEP (DENSE_RANK FIRST|LAST ORDER BY ...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeepDenseRankSlot {
    /// `KEEP (DENSE_RANK |)` - only `FIRST`/`LAST`.
    AfterDenseRank,
    /// `KEEP (DENSE_RANK FIRST|LAST |)` - only `ORDER`.
    AfterRankDirection,
}

/// Keyword position in an ordered-set aggregate `... WITHIN GROUP (...)` tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WithinGroupSlot {
    /// `<ordered-set-call> WITHIN |` - only `GROUP`.
    AfterWithin,
    /// `<ordered-set-call> WITHIN GROUP |` - only `(`, so no keyword candidates.
    AfterGroup,
}

/// Keyword position in an analytic null-treatment tail:
/// `FIRST_VALUE(...) IGNORE NULLS OVER (...)`,
/// `NTH_VALUE(...) FROM LAST RESPECT NULLS OVER (...)`, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnalyticNullTreatmentSlot {
    /// `<analytic-call> |` - `IGNORE`/`RESPECT`, or `FROM` for `NTH_VALUE`.
    AfterAnalyticCall,
    /// `NTH_VALUE(...) |` - may also take `FROM FIRST|LAST`.
    AfterNthValueCall,
    /// `NTH_VALUE(...) FROM |` - only `FIRST`/`LAST`.
    AfterNthValueFrom,
    /// `NTH_VALUE(...) FROM FIRST|LAST |` - null treatment or `OVER`.
    AfterNthValueFromDirection,
    /// `<analytic-tail> IGNORE|RESPECT |` - only `NULLS`.
    AfterNullTreatment,
    /// `<analytic-tail> IGNORE|RESPECT NULLS |` - only `OVER`.
    AfterNulls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnalyticNullTreatmentCall {
    General,
    NthValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedObjectSuggestionKind {
    Any,
    Routine,
    Executable,
    RelationOrSequence,
    Table,
    View,
    MaterializedView,
    Type,
    Trigger,
    Event,
    Index,
    Procedure,
    Function,
    Package,
    Sequence,
    Synonym,
    PublicSynonym,
    DatabaseLink,
    Directory,
    Library,
    Cluster,
    Context,
    Dimension,
    Operator,
    Indextype,
    Edition,
    JavaSource,
    JavaClass,
    JavaResource,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClauseCompletionPolicy {
    restrict_to_relation_columns: bool,
    select_list_wildcard_mode: SelectListWildcardMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoJoinRelationSegment {
    text: String,
    quoted_dotted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoJoinTableMatchKey {
    full: String,
    short: String,
    allow_short_match: bool,
}

impl ClauseCompletionPolicy {
    fn for_phase(phase: intellisense_context::SqlPhase, has_qualifier: bool) -> Self {
        let restrict_to_relation_columns = matches!(
            phase,
            intellisense_context::SqlPhase::CteColumnList
                | intellisense_context::SqlPhase::DerivedAliasColumnList
                | intellisense_context::SqlPhase::ConflictTargetList
                | intellisense_context::SqlPhase::JoinUsingColumnList
                | intellisense_context::SqlPhase::RecursiveCteColumnList
                | intellisense_context::SqlPhase::DmlSetTargetList
                | intellisense_context::SqlPhase::InsertColumnList
                | intellisense_context::SqlPhase::MergeInsertColumnList
                | intellisense_context::SqlPhase::DmlReturningList
                | intellisense_context::SqlPhase::LockingColumnList
        );
        let select_list_wildcard_mode =
            if matches!(phase, intellisense_context::SqlPhase::SelectList) {
                if has_qualifier {
                    SelectListWildcardMode::Qualified
                } else {
                    SelectListWildcardMode::Unqualified
                }
            } else {
                SelectListWildcardMode::None
            };

        Self {
            restrict_to_relation_columns,
            select_list_wildcard_mode,
        }
    }
}

/// Datetime field keywords valid in `EXTRACT(<field> FROM …)` for the dialect.
fn extract_field_keywords_for(
    db_type: Option<crate::db::DatabaseType>,
) -> &'static [&'static str] {
    use crate::db::DatabaseType;

    const ORACLE_FIELDS: &[&str] = &[
        "YEAR",
        "MONTH",
        "DAY",
        "HOUR",
        "MINUTE",
        "SECOND",
        "TIMEZONE_HOUR",
        "TIMEZONE_MINUTE",
        "TIMEZONE_REGION",
        "TIMEZONE_ABBR",
    ];
    const MYSQL_FIELDS: &[&str] = &[
        "MICROSECOND",
        "SECOND",
        "MINUTE",
        "HOUR",
        "DAY",
        "WEEK",
        "MONTH",
        "QUARTER",
        "YEAR",
        "SECOND_MICROSECOND",
        "MINUTE_MICROSECOND",
        "MINUTE_SECOND",
        "HOUR_MICROSECOND",
        "HOUR_SECOND",
        "HOUR_MINUTE",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
        "YEAR_MONTH",
    ];

    match db_type {
        None => ORACLE_FIELDS,
        Some(DatabaseType::Oracle) => ORACLE_FIELDS,
        Some(DatabaseType::MySQL) => MYSQL_FIELDS,
        Some(DatabaseType::MariaDB) => MYSQL_FIELDS,
    }
}

/// Keyword set offered at an `INTERVAL` qualifier slot. `TO` and the trailing
/// units are Oracle-only (MySQL has no `TO` in an interval literal); the
/// leading slot offers the dialect's unit names.
fn interval_unit_keywords_for(
    db_type: Option<crate::db::DatabaseType>,
    slot: IntervalUnitSlot,
) -> &'static [&'static str] {
    use crate::db::DatabaseType;

    const ORACLE_LEADING_UNITS: &[&str] = &["YEAR", "MONTH", "DAY", "HOUR", "MINUTE", "SECOND"];
    // Units valid after `TO`: `YEAR TO MONTH`, `DAY/HOUR/MINUTE TO {…SECOND}`.
    const ORACLE_TRAILING_UNITS: &[&str] = &["MONTH", "HOUR", "MINUTE", "SECOND"];
    const TO_KEYWORD: &[&str] = &["TO"];

    match slot {
        IntervalUnitSlot::Leading => match db_type {
            None => ORACLE_LEADING_UNITS,
            Some(DatabaseType::Oracle) => ORACLE_LEADING_UNITS,
            // MySQL's quoted interval reuses the same datetime unit names as
            // EXTRACT (including compound units like `DAY_HOUR`).
            Some(DatabaseType::MySQL) => extract_field_keywords_for(db_type),
            Some(DatabaseType::MariaDB) => extract_field_keywords_for(db_type),
        },
        IntervalUnitSlot::AwaitingTo => match db_type {
            None => TO_KEYWORD,
            Some(DatabaseType::Oracle) => TO_KEYWORD,
            // No `TO` qualifier in a MySQL interval — suppress columns but offer
            // nothing rather than a wrong keyword.
            Some(DatabaseType::MySQL) => &[],
            Some(DatabaseType::MariaDB) => &[],
        },
        IntervalUnitSlot::Trailing => ORACLE_TRAILING_UNITS,
    }
}

fn order_by_sort_modifier_keywords(slot: OrderBySortModifierSlot) -> &'static [&'static str] {
    match slot {
        OrderBySortModifierSlot::AfterDirection => &["NULLS"],
        OrderBySortModifierSlot::AfterNulls => &["FIRST", "LAST"],
        OrderBySortModifierSlot::AfterNullOrdering => &[],
    }
}

fn window_order_by_sort_modifier_keywords(
    slot: WindowOrderBySortModifierSlot,
) -> &'static [&'static str] {
    match slot {
        WindowOrderBySortModifierSlot::AfterSortKey => {
            &["ASC", "DESC", "NULLS", "ROWS", "RANGE", "GROUPS"]
        }
        WindowOrderBySortModifierSlot::AfterDirection => &["NULLS", "ROWS", "RANGE", "GROUPS"],
        WindowOrderBySortModifierSlot::AfterNulls => &["FIRST", "LAST"],
        WindowOrderBySortModifierSlot::AfterNullOrdering => &["ROWS", "RANGE", "GROUPS"],
    }
}

fn window_frame_keywords_for(slot: WindowFrameKeywordSlot) -> &'static [&'static str] {
    match slot {
        WindowFrameKeywordSlot::AfterUnit => &["BETWEEN", "UNBOUNDED", "CURRENT"],
        WindowFrameKeywordSlot::AfterBetween | WindowFrameKeywordSlot::AfterAnd => {
            &["UNBOUNDED", "CURRENT"]
        }
        WindowFrameKeywordSlot::AfterBoundExpression => &["PRECEDING", "FOLLOWING"],
        WindowFrameKeywordSlot::AfterUnbounded => &["PRECEDING", "FOLLOWING"],
        WindowFrameKeywordSlot::AfterCurrent => &["ROW"],
        WindowFrameKeywordSlot::AfterFirstBound => &["AND"],
        WindowFrameKeywordSlot::AfterFrameEnd => &["EXCLUDE"],
        WindowFrameKeywordSlot::AfterExclude => &["CURRENT", "GROUP", "TIES", "NO"],
        WindowFrameKeywordSlot::AfterExcludeCurrent => &["ROW"],
        WindowFrameKeywordSlot::AfterExcludeNo => &["OTHERS"],
        WindowFrameKeywordSlot::AfterExcludeEnd => &[],
    }
}

fn window_frame_slot_suppresses_columns(slot: WindowFrameKeywordSlot) -> bool {
    match slot {
        WindowFrameKeywordSlot::AfterUnit
        | WindowFrameKeywordSlot::AfterBetween
        | WindowFrameKeywordSlot::AfterAnd => false,
        _ => true,
    }
}

fn keep_dense_rank_keywords(slot: KeepDenseRankSlot) -> &'static [&'static str] {
    match slot {
        KeepDenseRankSlot::AfterDenseRank => &["FIRST", "LAST"],
        KeepDenseRankSlot::AfterRankDirection => &["ORDER"],
    }
}

fn within_group_keywords(slot: WithinGroupSlot) -> &'static [&'static str] {
    match slot {
        WithinGroupSlot::AfterWithin => &["GROUP"],
        WithinGroupSlot::AfterGroup => &[],
    }
}

fn analytic_null_treatment_keywords(
    slot: AnalyticNullTreatmentSlot,
) -> &'static [&'static str] {
    match slot {
        AnalyticNullTreatmentSlot::AfterAnalyticCall => &["IGNORE", "RESPECT", "OVER"],
        AnalyticNullTreatmentSlot::AfterNthValueCall => &["FROM", "IGNORE", "RESPECT", "OVER"],
        AnalyticNullTreatmentSlot::AfterNthValueFrom => &["FIRST", "LAST"],
        AnalyticNullTreatmentSlot::AfterNthValueFromDirection => &["IGNORE", "RESPECT", "OVER"],
        AnalyticNullTreatmentSlot::AfterNullTreatment => &["NULLS"],
        AnalyticNullTreatmentSlot::AfterNulls => &["OVER"],
    }
}

fn analytic_null_treatment_slot_suppresses_columns(
    slot: AnalyticNullTreatmentSlot,
) -> bool {
    !matches!(
        slot,
        AnalyticNullTreatmentSlot::AfterAnalyticCall
            | AnalyticNullTreatmentSlot::AfterNthValueCall
    )
}

/// Data-type keyword set for a dialect and position. MySQL/MariaDB restrict
/// the `CAST(... AS type)` grammar to a subset, so that position differs from
/// a full column definition; Oracle uses one list for both.
fn data_type_keywords_for(
    db_type: Option<crate::db::DatabaseType>,
    position: DataTypePosition,
) -> &'static [&'static str] {
    use crate::db::DatabaseType;

    match db_type {
        None => oracle_data_type_keywords(position),
        Some(DatabaseType::Oracle) => oracle_data_type_keywords(position),
        Some(DatabaseType::MySQL) => mysql_data_type_keywords(position),
        Some(DatabaseType::MariaDB) => mysql_data_type_keywords(position),
    }
}

fn oracle_data_type_keywords(position: DataTypePosition) -> &'static [&'static str] {
    const ORACLE_TYPES: &[&str] = &[
        "VARCHAR2",
        "NVARCHAR2",
        "CHAR",
        "NCHAR",
        "NUMBER",
        "FLOAT",
        "BINARY_FLOAT",
        "BINARY_DOUBLE",
        "DATE",
        "TIMESTAMP",
        "INTERVAL",
        "CLOB",
        "NCLOB",
        "BLOB",
        "BFILE",
        "RAW",
        "LONG",
        "ROWID",
        "UROWID",
        "XMLTYPE",
        "JSON",
        "BOOLEAN",
        "INTEGER",
        "INT",
        "SMALLINT",
        "DECIMAL",
        "NUMERIC",
        "REAL",
    ];
    // PL/SQL adds a handful of types that exist only in stored code.
    const ORACLE_PLSQL_TYPES: &[&str] = &[
        "VARCHAR2",
        "NVARCHAR2",
        "CHAR",
        "NCHAR",
        "NUMBER",
        "PLS_INTEGER",
        "BINARY_INTEGER",
        "SIMPLE_INTEGER",
        "BINARY_FLOAT",
        "BINARY_DOUBLE",
        "FLOAT",
        "DATE",
        "TIMESTAMP",
        "INTERVAL",
        "CLOB",
        "NCLOB",
        "BLOB",
        "BFILE",
        "RAW",
        "ROWID",
        "UROWID",
        "XMLTYPE",
        "BOOLEAN",
        "INTEGER",
        "INT",
        "SMALLINT",
        "DECIMAL",
        "NUMERIC",
        "REAL",
        "SYS_REFCURSOR",
    ];

    match position {
        DataTypePosition::Plsql => ORACLE_PLSQL_TYPES,
        _ => ORACLE_TYPES,
    }
}

fn mysql_data_type_keywords(position: DataTypePosition) -> &'static [&'static str] {
    const MYSQL_COLUMN_TYPES: &[&str] = &[
        "TINYINT",
        "SMALLINT",
        "MEDIUMINT",
        "INT",
        "INTEGER",
        "BIGINT",
        "DECIMAL",
        "NUMERIC",
        "FLOAT",
        "DOUBLE",
        "BIT",
        "BOOLEAN",
        "CHAR",
        "VARCHAR",
        "BINARY",
        "VARBINARY",
        "TINYBLOB",
        "BLOB",
        "MEDIUMBLOB",
        "LONGBLOB",
        "TINYTEXT",
        "TEXT",
        "MEDIUMTEXT",
        "LONGTEXT",
        "ENUM",
        "SET",
        "DATE",
        "DATETIME",
        "TIMESTAMP",
        "TIME",
        "YEAR",
        "JSON",
    ];
    // The grammar that `CAST(expr AS type)` accepts in MySQL/MariaDB.
    const MYSQL_CAST_TYPES: &[&str] = &[
        "BINARY",
        "CHAR",
        "DATE",
        "DATETIME",
        "DECIMAL",
        "DOUBLE",
        "FLOAT",
        "JSON",
        "NCHAR",
        "REAL",
        "SIGNED",
        "TIME",
        "UNSIGNED",
        "YEAR",
    ];

    match position {
        DataTypePosition::Cast => MYSQL_CAST_TYPES,
        DataTypePosition::ColumnDef | DataTypePosition::Plsql => MYSQL_COLUMN_TYPES,
    }
}

impl SqlEditorWidget {
    fn context_suppresses_completion(context: SqlContext) -> bool {
        matches!(context, SqlContext::GeneratedName)
    }

    pub(super) fn trigger_intellisense(
        editor: &TextEditor,
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) {
        let request_generation = runtime.next_parse_generation();
        let buffer_revision = runtime.current_buffer_revision();
        let (cursor_pos, cursor_pos_usize) = Self::editor_cursor_position(editor, buffer);
        let (prefix, word_start, _) = Self::word_at_cursor(buffer, text_shadow, cursor_pos);
        let qualifier = Self::qualifier_before_word(buffer, text_shadow, word_start);
        let raw_qualifier = Self::raw_qualifier_before_word(buffer, text_shadow, word_start);
        // Avoid blocking the UI thread on the connection mutex (which the
        // schema refresh worker or a running query may be holding). Fall back
        // to the last observed db_type; it only changes on (re)connect.
        let preferred_db_type = match connection.try_lock() {
            Ok(conn_guard) => {
                let db_type = conn_guard.db_type();
                runtime.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let db_type = poisoned.into_inner().db_type();
                runtime.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::WouldBlock) => runtime.cached_db_type(),
        };
        let should_hide_after_statement_terminator = prefix.is_empty()
            && qualifier.is_none()
            && Self::non_whitespace_char_before_cursor(buffer, text_shadow, cursor_pos)
                == Some(';');

        // The cursor sitting inside a string literal or comment is never an
        // identifier position: keywords, columns and relations would all be
        // irrelevant there. Suppress completion uniformly for every clause by
        // reusing the syntax highlighter's already-computed styles.
        let cursor_in_string_or_comment = text_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor_in_string_or_comment(cursor_pos_usize);

        if should_hide_after_statement_terminator || cursor_in_string_or_comment {
            intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            runtime.clear_ui_tracking();
            return;
        }

        let snapshot = Arc::new(IntellisenseTriggerSnapshot {
            request_generation,
            buffer_revision,
            cursor_pos,
            cursor_pos_usize,
            preferred_db_type,
            prefix,
            word_start,
            qualifier,
            raw_qualifier,
        });

        let cached_context = runtime.parse_cache().and_then(|entry| {
            (entry.buffer_revision == snapshot.buffer_revision
                && entry.cursor_pos == snapshot.cursor_pos)
                .then_some(entry.analysis.clone())
        });

        if let Some(analysis) = cached_context {
            Self::apply_intellisense_with_context(
                editor,
                intellisense_data,
                intellisense_popup,
                column_sender,
                connection,
                runtime,
                snapshot.as_ref(),
                analysis.as_ref(),
            );
            return;
        }

        if let Some(routine_cache) = runtime.routine_symbol_cache_covering_cursor(
            snapshot.buffer_revision,
            snapshot.cursor_pos_usize,
        ) {
            let cursor_in_statement = snapshot
                .cursor_pos_usize
                .saturating_sub(routine_cache.statement_start)
                .min(
                    routine_cache
                        .statement_end
                        .saturating_sub(routine_cache.statement_start),
                );
            let analysis = Arc::new(Self::build_intellisense_analysis_from_routine_cache(
                &routine_cache,
                cursor_in_statement,
            ));
            runtime.set_parse_cache(Some(IntellisenseParseCacheEntry {
                buffer_revision: snapshot.buffer_revision,
                cursor_pos: snapshot.cursor_pos,
                analysis: analysis.clone(),
            }));
            Self::apply_intellisense_with_context(
                editor,
                intellisense_data,
                intellisense_popup,
                column_sender,
                connection,
                runtime,
                snapshot.as_ref(),
                analysis.as_ref(),
            );
            return;
        }

        // Cache miss means full parse is pending on a worker.
        // Hide stale popup/completion state to avoid applying outdated candidates.
        Self::clear_intellisense_ui_state(intellisense_popup, runtime);

        Self::queue_async_intellisense_parse(
            editor,
            text_shadow,
            intellisense_data,
            intellisense_popup,
            column_sender,
            connection,
            runtime,
            snapshot.clone(),
        );
    }

    #[cfg(test)]
    fn analyze_statement_context(
        statement_text: &str,
        cursor_in_statement: usize,
    ) -> intellisense_context::CursorContext {
        type CachedStatementTokens = (Arc<[usize]>, Arc<[SqlToken]>);

        static TOKENIZED_STATEMENT_CACHE: OnceLock<Mutex<HashMap<String, CachedStatementTokens>>> =
            OnceLock::new();

        let cache = TOKENIZED_STATEMENT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let (token_ends, statement_tokens) = {
            let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(entry) = guard.get(statement_text) {
                entry.clone()
            } else {
                let full_token_spans = super::query_text::tokenize_sql_spanned(statement_text);
                let token_ends: Arc<[usize]> = full_token_spans
                    .iter()
                    .map(|span| span.end)
                    .collect::<Vec<_>>()
                    .into();
                let statement_tokens: Arc<[SqlToken]> = full_token_spans
                    .into_iter()
                    .map(|span| span.token)
                    .collect::<Vec<_>>()
                    .into();
                let entry = (token_ends, statement_tokens);
                guard.insert(statement_text.to_string(), entry.clone());
                entry
            }
        };
        let split_idx = token_ends.partition_point(|end| *end <= cursor_in_statement);
        intellisense_context::analyze_cursor_context_arc(statement_tokens, split_idx)
    }

    fn is_intellisense_snapshot_current(
        editor: &TextEditor,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
    ) -> bool {
        if editor.was_deleted() {
            return false;
        }

        if editor.insert_position() != snapshot.cursor_pos {
            return false;
        }

        runtime.current_buffer_revision() == snapshot.buffer_revision
    }

    fn is_intellisense_parse_generation_current(
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
    ) -> bool {
        runtime.current_parse_generation() == snapshot.request_generation
    }

    fn clear_intellisense_ui_state(
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) {
        intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
        runtime.clear_ui_tracking();
    }

    fn clear_intellisense_state_for_external_hide(runtime: &Arc<IntellisenseRuntimeState>) {
        Self::invalidate_and_clear_pending_intellisense_state(runtime);
    }

    fn should_ignore_external_hide_click(popup_visible: bool, click_inside_popup: bool) -> bool {
        popup_visible && click_inside_popup
    }

    fn should_hide_popup_on_unfocus(popup_visible: bool, pointer_inside_popup: bool) -> bool {
        popup_visible && !pointer_inside_popup
    }

    fn schedule_deferred_unfocus_popup_hide(
        editor: TextEditor,
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        pointer_x: i32,
        pointer_y: i32,
        retries_left: u8,
    ) {
        app::add_timeout3(0.0, move |_| {
            if editor.was_deleted() {
                return;
            }

            if matches!(
                runtime.popup_transition_state(),
                IntellisensePopupTransitionState::Showing
            ) {
                if retries_left > 0 {
                    Self::schedule_deferred_unfocus_popup_hide(
                        editor.clone(),
                        intellisense_popup.clone(),
                        runtime.clone(),
                        pointer_x,
                        pointer_y,
                        retries_left - 1,
                    );
                }
                return;
            }

            if editor.has_focus() {
                return;
            }
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let popup_visible = popup.is_visible();
            let pointer_inside_popup = popup_visible && popup.contains_point(pointer_x, pointer_y);
            if !Self::should_hide_popup_on_unfocus(popup_visible, pointer_inside_popup) {
                return;
            }
            popup.hide();
            drop(popup);
            Self::clear_intellisense_state_for_external_hide(&runtime);
        });
    }

    fn schedule_deferred_outside_click_popup_hide(
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        click_x: i32,
        click_y: i32,
        retries_left: u8,
    ) {
        app::add_timeout3(0.0, move |_| {
            if matches!(
                runtime.popup_transition_state(),
                IntellisensePopupTransitionState::Showing
            ) {
                if retries_left > 0 {
                    Self::schedule_deferred_outside_click_popup_hide(
                        intellisense_popup.clone(),
                        runtime.clone(),
                        click_x,
                        click_y,
                        retries_left - 1,
                    );
                }
                return;
            }
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let popup_visible = popup.is_visible();
            if !popup_visible {
                return;
            }
            let click_inside_popup = popup.contains_point(click_x, click_y);
            if Self::should_ignore_external_hide_click(popup_visible, click_inside_popup) {
                return;
            }
            popup.hide();
            drop(popup);
            Self::clear_intellisense_state_for_external_hide(&runtime);
        });
    }

    fn invalidate_and_clear_pending_intellisense_state(runtime: &Arc<IntellisenseRuntimeState>) {
        runtime.clear_ui_tracking();
        Self::invalidate_keyup_debounce_with_parse_generation(runtime, true);
    }

    fn cancel_intellisense_on_escape_keydown(
        popup_visible: bool,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) -> bool {
        Self::invalidate_and_clear_pending_intellisense_state(runtime);
        popup_visible
    }

    fn queue_async_intellisense_parse(
        editor: &TextEditor,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: Arc<IntellisenseTriggerSnapshot>,
    ) {
        let (parse_sender, parse_receiver) =
            mpsc::channel::<Result<AsyncIntellisenseParseResult, String>>();
        let parse_receiver = Arc::new(Mutex::new(parse_receiver));
        let snapshot_for_thread = snapshot.clone();
        let text_shadow_for_thread = text_shadow.clone();
        let routine_symbol_cache_for_thread = runtime.routine_symbol_cache_handle();
        let spawn_result = thread::Builder::new()
            .name("intellisense-parse-worker".to_string())
            .spawn(move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        let (expanded_statement, text_bind_names) =
                            Self::expanded_statement_window_and_text_binds_from_shadow(
                                &text_shadow_for_thread,
                                snapshot_for_thread.cursor_pos_usize,
                                Some(snapshot_for_thread.preferred_db_type),
                            );
                    let routine_cache = {
                        let cache = routine_symbol_cache_for_thread
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        cache
                            .iter()
                            .find(|entry| {
                                entry.buffer_revision == snapshot_for_thread.buffer_revision
                                    && entry.statement_start == expanded_statement.statement_start
                                    && entry.statement_end == expanded_statement.statement_end
                            })
                            .cloned()
                    }
                    .unwrap_or_else(|| {
                        Self::build_routine_symbol_cache_entry(
                            snapshot_for_thread.buffer_revision,
                            &expanded_statement,
                            text_bind_names,
                        )
                    });
                    let analysis = Self::build_intellisense_analysis_from_routine_cache(
                        &routine_cache,
                        expanded_statement.cursor_in_statement,
                    );

                    AsyncIntellisenseParseResult {
                        analysis,
                        routine_cache,
                    }
                }));

                match result {
                    Ok(parsed) => {
                        let _ = parse_sender.send(Ok(parsed));
                    }
                    Err(payload) => {
                        let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                        crate::utils::logging::log_error(
                            "sql_editor::intellisense::parse_worker",
                            &format!("parse worker panicked: {panic_msg}"),
                        );
                        let _ = parse_sender.send(Err(format!("Internal error: {panic_msg}")));
                    }
                }
                app::awake();
            });

        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::parse_worker",
                &format!("failed to spawn parse worker: {err}"),
            );
            if Self::is_intellisense_parse_generation_current(runtime, snapshot.as_ref())
                && Self::is_intellisense_snapshot_current(editor, runtime, snapshot.as_ref())
            {
                Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            }
            return;
        }

        let editor_for_poll = editor.clone();
        let intellisense_data_for_poll = intellisense_data.clone();
        let intellisense_popup_for_poll = intellisense_popup.clone();
        let column_sender_for_poll = column_sender.clone();
        let connection_for_poll = connection.clone();
        let runtime_for_poll = runtime.clone();
        app::add_timeout3(0.0, move |_| {
            Self::poll_async_intellisense_parse(
                editor_for_poll.clone(),
                intellisense_data_for_poll.clone(),
                intellisense_popup_for_poll.clone(),
                column_sender_for_poll.clone(),
                connection_for_poll.clone(),
                runtime_for_poll.clone(),
                snapshot.clone(),
                parse_receiver.clone(),
            );
        });
    }

    fn poll_async_intellisense_parse(
        editor: TextEditor,
        intellisense_data: Arc<Mutex<IntellisenseData>>,
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        column_sender: mpsc::Sender<ColumnLoadUpdate>,
        connection: SharedConnection,
        runtime: Arc<IntellisenseRuntimeState>,
        snapshot: Arc<IntellisenseTriggerSnapshot>,
        parse_receiver: Arc<Mutex<mpsc::Receiver<Result<AsyncIntellisenseParseResult, String>>>>,
    ) {
        if editor.was_deleted() {
            return;
        }
        if !Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref()) {
            return;
        }

        let recv_result = {
            let receiver = parse_receiver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            receiver.try_recv()
        };

        match recv_result {
            Ok(Ok(parsed)) => {
                if !Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    || !Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    return;
                }
                runtime.set_routine_symbol_cache(parsed.routine_cache.clone());
                let parsed = Arc::new(parsed.analysis);
                runtime.set_parse_cache(Some(IntellisenseParseCacheEntry {
                    buffer_revision: snapshot.buffer_revision,
                    cursor_pos: snapshot.cursor_pos,
                    analysis: parsed.clone(),
                }));

                Self::apply_intellisense_with_context(
                    &editor,
                    &intellisense_data,
                    &intellisense_popup,
                    &column_sender,
                    &connection,
                    &runtime,
                    snapshot.as_ref(),
                    parsed.as_ref(),
                );
            }
            Ok(Err(message)) => {
                crate::utils::logging::log_error(
                    "sql_editor::intellisense::parse_worker",
                    &format!("failed to parse intellisense context: {message}"),
                );
                if Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    && Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    Self::clear_intellisense_ui_state(&intellisense_popup, &runtime);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                app::add_timeout3(INTELLISENSE_PARSE_POLL_INTERVAL_SECONDS, move |_| {
                    Self::poll_async_intellisense_parse(
                        editor.clone(),
                        intellisense_data.clone(),
                        intellisense_popup.clone(),
                        column_sender.clone(),
                        connection.clone(),
                        runtime.clone(),
                        snapshot.clone(),
                        parse_receiver.clone(),
                    );
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                if Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    && Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    Self::clear_intellisense_ui_state(&intellisense_popup, &runtime);
                }
            }
        }
    }

    fn apply_intellisense_with_context(
        editor: &TextEditor,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
        analysis: &IntellisenseAnalysis,
    ) {
        let deep_ctx = analysis.context.as_ref();
        let qualifier = snapshot.qualifier.as_deref();
        let context =
            Self::classify_intellisense_context(deep_ctx, deep_ctx.statement_tokens.as_ref());
        // An alias declaration (`t AS x` / `t x` / `[x]`) or a DDL new-name /
        // data-type slot (`ALTER TABLE t ADD col …`, `RENAME … TO new`) names a
        // brand-new identifier; offering keywords, columns or relations there is
        // always irrelevant. Suppress uniformly, regardless of the clause.
        if Self::context_suppresses_completion(context)
            || (qualifier.is_none() && analysis.cursor_in_alias_declaration)
            || (qualifier.is_none() && deep_ctx.ddl_new_name_position)
            || (qualifier.is_none()
                && Self::cursor_is_at_select_list_alias_name_slot(
                    deep_ctx,
                    !snapshot.prefix.is_empty(),
                ))
            || (qualifier.is_none()
                && Self::cursor_is_at_create_object_new_name(
                    deep_ctx,
                    !snapshot.prefix.is_empty(),
                ))
        {
            Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            return;
        }
        let completion_policy =
            ClauseCompletionPolicy::for_phase(deep_ctx.phase, qualifier.is_some());
        let restrict_to_relation_columns = completion_policy.restrict_to_relation_columns;
        // A keyword-only position accepts only a fixed keyword (or, for an alias
        // slot, a brand-new name), never an existing identifier — a clause-keyword
        // continuation (`ORDER |`/`GROUP |`/`<join-type> |` …), an ORDER BY sort
        // modifier tail (`ASC |` → `NULLS`, `NULLS |` → `FIRST`/`LAST`), the
        // `IS [NOT] |` null-test operator, the slot right after a complete DML
        // target table (`UPDATE t |` → `SET`, …), the slot right after a complete
        // JOIN target table (`… JOIN t |` → `ON`/`USING`), or a table-clause
        // alias-name slot
        // (`FROM t AS |`). The phase machine leaves the cursor in the surrounding
        // table/column phase there, so every identifier source (relations,
        // columns, in-scope aliases/CTEs, local PL/SQL symbols, `*`) must be
        // suppressed; the `expected_keyword_suggestions` merge below still supplies
        // the lone `BY`/`WITH`/`JOIN`/`NULL`/`SET`/`WHERE`/`ON`/… hints (an alias
        // slot has none, so its popup simply stays empty). The keyword-
        // emitting slots are also folded into `at_keyword_only_slot` (via the
        // shared `cursor_is_at_column_suppressing_keyword_slot` chokepoint) so
        // column-gated paths stay consistent.
        let has_prefix = !snapshot.prefix.is_empty();
        let at_keyword_only_identifier_slot = qualifier.is_none()
            && (Self::cursor_is_at_pure_clause_keyword_continuation_for_context(
                deep_ctx, has_prefix,
            ) || Self::order_by_sort_modifier_slot_for_context(deep_ctx, has_prefix).is_some()
                || Self::window_order_by_sort_modifier_slot_for_context(deep_ctx, has_prefix)
                    .is_some()
                || Self::expected_window_spec_clause_transition_candidates_for_context(
                    deep_ctx, has_prefix,
                )
                .is_some()
                || Self::keep_dense_rank_slot_for_context(deep_ctx, has_prefix).is_some()
                || Self::within_group_slot_for_context(deep_ctx, has_prefix).is_some()
                || Self::analytic_null_treatment_slot_for_context(deep_ctx, has_prefix)
                    .is_some_and(analytic_null_treatment_slot_suppresses_columns)
                || Self::cursor_is_at_is_null_test_keyword_position_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_after_complete_dml_target_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_after_complete_join_target_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_at_table_alias_name_slot(deep_ctx, has_prefix)
                || Self::cursor_is_at_merge_then_action_slot_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_at_merge_when_keyword_slot_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_after_set_operator_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_after_complete_from_relation_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_after_complete_alter_table_target_for_context(deep_ctx, has_prefix)
                || Self::cursor_is_at_locking_clause_keyword_slot_for_context(deep_ctx, has_prefix)
                || Self::expected_sounds_like_keyword_candidates_for_context(
                    deep_ctx,
                    has_prefix,
                    Some(snapshot.preferred_db_type),
                )
                .is_some());
        let cursor_in_statement = snapshot
            .cursor_pos_usize
            .saturating_sub(analysis.statement_start)
            .min(
                analysis
                    .statement_end
                    .saturating_sub(analysis.statement_start),
            );
        let session_bind_names = if qualifier.is_none()
            && !at_keyword_only_identifier_slot
            && !matches!(context, SqlContext::TableName)
            && !restrict_to_relation_columns
        {
            Self::session_bind_names(connection)
        } else {
            Vec::new()
        };
        let local_suggestions = if qualifier.is_none()
            && !at_keyword_only_identifier_slot
            && !matches!(context, SqlContext::TableName)
            && !restrict_to_relation_columns
        {
            Self::collect_local_symbol_suggestions(
                &snapshot.prefix,
                cursor_in_statement,
                analysis,
                &session_bind_names,
            )
        } else {
            Vec::new()
        };
        let local_record_member_suggestions = qualifier
            .and_then(|qualifier| {
                Self::collect_local_record_member_suggestions(
                    qualifier,
                    &snapshot.prefix,
                    cursor_in_statement,
                    snapshot.raw_qualifier.as_deref(),
                    analysis,
                )
            });
        let has_resolved_local_record_member_scope = local_record_member_suggestions.is_some();
        let local_rowtype_member_sources = qualifier
            .map(|qualifier| {
                Self::local_rowtype_member_sources_for_qualifier(
                    qualifier,
                    cursor_in_statement,
                    snapshot.raw_qualifier.as_deref(),
                    analysis,
                )
            })
            .unwrap_or_default();
        for source in &local_rowtype_member_sources {
            Self::request_table_columns(source, intellisense_data, column_sender, connection);
        }
        let local_rowtype_member_suggestions = if !local_rowtype_member_sources.is_empty() {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get_column_suggestions(&snapshot.prefix, Some(&local_rowtype_member_sources))
        } else {
            Vec::new()
        };
        let has_local_record_member_scope =
            has_resolved_local_record_member_scope || !local_rowtype_member_sources.is_empty();
        let local_record_member_suggestions =
            local_record_member_suggestions.unwrap_or_default();
        let local_rowtype_column_tables = local_rowtype_member_sources.clone();
        let qualified_completion_mode = if has_local_record_member_scope {
            None
        } else {
            qualifier.and_then(|qualifier| {
                let data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::resolve_qualified_completion_mode(qualifier, context, deep_ctx, &data)
            })
        };
        let qualified_mode_uses_members = matches!(
            qualified_completion_mode,
            Some(QualifiedCompletionMode::RelationMembers | QualifiedCompletionMode::ObjectMembers)
        );
        let column_tables = if has_local_record_member_scope || qualified_mode_uses_members {
            Vec::new()
        } else {
            Self::resolve_column_tables_for_context(qualifier, deep_ctx)
        };
        // The cursor may sit in a slot whose grammar is keyword/value-only — a
        // data type, a row-limiting argument, a pure window-frame keyword, an
        // EXTRACT field, an INTERVAL unit — where a column is never valid.
        // `cursor_is_at_column_suppressing_keyword_slot` is the single place those
        // positions are enumerated, so column suppression cannot drift away from
        // the matching keyword hints `collect_expected_keyword_suggestions` emits.
        let at_keyword_only_slot = qualifier.is_none()
            && Self::cursor_is_at_column_suppressing_keyword_slot_for_db(
                deep_ctx,
                !snapshot.prefix.is_empty(),
                Some(snapshot.preferred_db_type),
            );
        // A data-type slot is the one keyword-only slot that still admits an
        // identifier — a user-defined TYPE object (`CAST(x AS my_type)`,
        // `col my_type`). It is handled specially below so types survive while
        // every other identifier source is still suppressed.
        let at_data_type_position = qualifier.is_none()
            && Self::data_type_position_for_context(deep_ctx, !snapshot.prefix.is_empty())
                .is_some();
        let expr_keyword_ctx = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::expression_keyword_context(
                deep_ctx,
                &data,
                &column_tables,
                !snapshot.prefix.is_empty(),
                Some(snapshot.preferred_db_type),
            )
        };
        let include_columns = !has_local_record_member_scope
            && !at_keyword_only_slot
            && (matches!(
                qualified_completion_mode,
                Some(QualifiedCompletionMode::RelationColumns)
            ) || (qualified_completion_mode.is_none()
                && (qualifier.is_some()
                    || matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll))));
        let comparison_lookup_tables = if has_local_record_member_scope || qualified_mode_uses_members
        {
            Vec::new()
        } else {
            Self::comparison_lookup_tables_for_context(qualifier, deep_ctx)
        };
        let qualified_member_suggestions = match (qualifier, qualified_completion_mode) {
            (Some(qualifier), Some(QualifiedCompletionMode::RelationMembers)) => {
                let mut data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::expected_relation_member_suggestions_for_qualifier(
                    &mut data,
                    qualifier,
                    &snapshot.prefix,
                    deep_ctx,
                )
            }
            (Some(qualifier), Some(QualifiedCompletionMode::ObjectMembers)) => {
                let mut data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::expected_member_suggestions_for_qualifier(
                    &mut data,
                    qualifier,
                    &snapshot.prefix,
                    deep_ctx,
                )
            }
            _ => Vec::new(),
        };
        let expected_keyword_suggestions = if qualifier.is_none()
            && !restrict_to_relation_columns
            && !matches!(context, SqlContext::VariableName | SqlContext::BindValue)
        {
            Self::collect_expected_keyword_suggestions(
                &snapshot.prefix,
                deep_ctx,
                Some(snapshot.preferred_db_type),
            )
        } else {
            Vec::new()
        };
        let expected_object_suggestions = if qualifier.is_none()
            && !restrict_to_relation_columns
            && !matches!(
                context,
                SqlContext::VariableName
                    | SqlContext::BindValue
                    | SqlContext::ColumnName
                    | SqlContext::ColumnOrAll
            ) {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::collect_expected_object_suggestions(&mut data, &snapshot.prefix, deep_ctx)
        } else {
            Vec::new()
        };
        // Replace (not merge) the base catalog with the kind-specific object
        // list whenever the slot admits only one object family: a table-context
        // slot, or any *constrained* DDL object slot (`DROP TRIGGER`, `CALL`,
        // `GRANT … ON`, `GRANT … TO`, `COMMENT ON`, `REFERENCES`, …). Most of
        // those classify as the neutral General phase, where the base would
        // otherwise append the whole catalog (every table, trigger, index,
        // directory, function) after the correct objects — pure noise. An `Any`
        // slot (`DESC`/`AUDIT`/`CREATE SYNONYM` target) keeps the full catalog
        // because every object kind is valid there.
        let expected_object_kind = if qualifier.is_none() {
            Self::expected_object_suggestion_kind(&snapshot.prefix, None, deep_ctx)
        } else {
            None
        };
        let replace_table_context_with_expected_objects = match expected_object_kind {
            Some(ExpectedObjectSuggestionKind::Any) => matches!(context, SqlContext::TableName),
            Some(_) => true,
            None => false,
        };

        let allow_empty_prefix = qualifier.is_some()
            || include_columns
            || matches!(context, SqlContext::TableName)
            || !local_suggestions.is_empty()
            || has_local_record_member_scope
            || !qualified_member_suggestions.is_empty()
            || !expected_keyword_suggestions.is_empty()
            || !expected_object_suggestions.is_empty();
        if snapshot.prefix.is_empty() && !allow_empty_prefix {
            // Context no longer allows completion for empty prefix, so hide
            // stale popup state immediately.
            Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            return;
        }

        let mut virtual_wildcard_dependencies: HashMap<String, Vec<String>> = HashMap::new();
        if include_columns {
            let mut virtual_table_columns: HashMap<String, Vec<String>> = HashMap::new();
            for cte in &deep_ctx.ctes {
                let (columns, wildcard_tables) = Self::collect_cte_virtual_columns_for_completion(
                    deep_ctx,
                    cte,
                    &virtual_table_columns,
                    intellisense_data,
                    column_sender,
                    connection,
                );
                if !wildcard_tables.is_empty() {
                    virtual_wildcard_dependencies.insert(cte.name.to_uppercase(), wildcard_tables);
                }
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut virtual_table_columns,
                        &cte.name,
                        columns,
                    );
                }
            }

            for subq in &deep_ctx.subqueries {
                if let Some(columns) =
                    Self::explicit_subquery_columns_for_completion(deep_ctx, subq)
                {
                    Self::insert_virtual_table_columns(
                        &mut virtual_table_columns,
                        &subq.alias,
                        columns,
                    );
                    continue;
                }
                let body_tokens = intellisense_context::token_range_slice(
                    deep_ctx.statement_tokens.as_ref(),
                    subq.body_range,
                );
                let body_ctx =
                    intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
                let mut body_virtual_table_columns = virtual_table_columns.clone();
                for cte in &body_ctx.ctes {
                    let (nested_columns, _) = Self::collect_cte_virtual_columns_for_completion(
                        &body_ctx,
                        cte,
                        &body_virtual_table_columns,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                    if !nested_columns.is_empty() {
                        Self::insert_virtual_table_columns(
                            &mut body_virtual_table_columns,
                            &cte.name,
                            nested_columns,
                        );
                    }
                }
                let body_local_tables =
                    intellisense_context::collect_tables_in_statement(body_tokens);
                let (columns, wildcard_tables) =
                    Self::collect_virtual_relation_columns_for_completion(
                        body_tokens,
                        &body_local_tables,
                        &deep_ctx.tables_in_scope,
                        &body_virtual_table_columns,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                if !wildcard_tables.is_empty() {
                    virtual_wildcard_dependencies
                        .insert(subq.alias.to_uppercase(), wildcard_tables);
                }
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut virtual_table_columns,
                        &subq.alias,
                        columns,
                    );
                }
            }
            intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .replace_virtual_table_columns(virtual_table_columns);

            for table in &column_tables {
                let is_virtual = deep_ctx
                    .ctes
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(table))
                    || deep_ctx
                        .subqueries
                        .iter()
                        .any(|s| s.alias.eq_ignore_ascii_case(table));
                if !is_virtual {
                    Self::request_table_columns(
                        table,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                }
            }

            for table in &comparison_lookup_tables {
                Self::request_table_columns(table, intellisense_data, column_sender, connection);
            }
        }

        let columns_loading = if include_columns {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let loading_tables = if comparison_lookup_tables.is_empty() {
                column_tables.clone()
            } else {
                let mut merged_tables = column_tables.clone();
                for table in &comparison_lookup_tables {
                    if merged_tables
                        .iter()
                        .all(|existing| !existing.eq_ignore_ascii_case(table))
                    {
                        merged_tables.push(table.clone());
                    }
                }
                merged_tables
            };
            Self::has_column_loading_for_scope(
                include_columns,
                &loading_tables,
                &virtual_wildcard_dependencies,
                &data,
            )
        } else if !local_rowtype_column_tables.is_empty() {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::has_column_loading_for_scope(
                true,
                &local_rowtype_column_tables,
                &HashMap::new(),
                &data,
            )
        } else {
            false
        };

        let mut suggestions = if has_resolved_local_record_member_scope {
            Self::merge_suggestions_with_context_aliases(
                local_record_member_suggestions,
                local_rowtype_member_suggestions,
                false,
            )
        } else if !local_rowtype_member_sources.is_empty() {
            local_rowtype_member_suggestions
        } else if !qualified_member_suggestions.is_empty() {
            qualified_member_suggestions
        } else if replace_table_context_with_expected_objects {
            expected_object_suggestions.clone()
        } else if at_data_type_position {
            // A data-type slot (`CAST(x AS |)`, a column/PL-SQL type) admits
            // only type names: the dialect type keywords come from the expected-
            // keyword merge below, and user-defined TYPE objects from here.
            // Relations, functions, columns and unrelated keywords are all
            // irrelevant, so the rest of the catalog stays suppressed.
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get_type_object_suggestions(&snapshot.prefix)
        } else if at_keyword_only_identifier_slot || at_keyword_only_slot {
            // A pure keyword/value-only slot: a clause-keyword continuation, an
            // ORDER BY sort modifier tail, `IS [NOT] NULL`, a complete DML/JOIN
            // target tail, a MERGE action, a `FOR`-locking keyword, an `EXTRACT`
            // field, an `INTERVAL` unit, a window-frame keyword, or a row-
            // limiting count. Only the slot's fixed keyword(s) are grammatical;
            // supplied by the keyword merge below — so the identifier base (every
            // relation, function, column and unrelated keyword) stays empty.
            Vec::new()
        } else {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let column_scope = if !column_tables.is_empty() {
                Some(column_tables.as_slice())
            } else {
                None
            };
            if qualifier.is_none()
                && matches!(
                    deep_ctx.phase,
                    intellisense_context::SqlPhase::JoinUsingColumnList
                )
            {
                Self::collect_common_column_suggestions(&snapshot.prefix, &column_tables, &data)
            } else {
                Self::base_suggestions_for_context(
                    &mut data,
                    &snapshot.prefix,
                    qualifier,
                    column_scope,
                    include_columns,
                    context,
                    restrict_to_relation_columns,
                    Some(snapshot.preferred_db_type),
                    expr_keyword_ctx,
                )
            }
        };
        if !expected_object_suggestions.is_empty() && !replace_table_context_with_expected_objects {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                expected_object_suggestions,
                true,
            );
        }
        if !expected_keyword_suggestions.is_empty() {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                expected_keyword_suggestions,
                true,
            );
        }
        let comparison_suggestions = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            qualifier
                .map(|qualifier| {
                    Self::collect_qualified_condition_comparison_suggestions(
                        &data,
                        &snapshot.prefix,
                        qualifier,
                        deep_ctx,
                    )
                })
                .unwrap_or_default()
        };
        if !comparison_suggestions.is_empty() {
            suggestions = Self::merge_qualified_condition_comparison_suggestions(
                suggestions,
                comparison_suggestions,
                deep_ctx.phase,
            );
        }
        // A column wildcard (`*` / `t.*`) is column material, so every
        // column-suppressing keyword-only slot must drop it just like the
        // identifier base does. `at_keyword_only_identifier_slot` covers the
        // clause-continuation slots; `at_keyword_only_slot` covers the
        // value/keyword-only slots enumerated at the suppression chokepoint
        // (EXTRACT field, data type, INTERVAL unit, window-frame bound,
        // row-limit count), where `EXTRACT(| FROM d)` previously leaked `*`.
        // It is also operand material, so it is dropped right after a complete
        // operand (`SELECT empno |`, `SELECT 'x' |`) where only an operator/
        // comma/`FROM` can follow; the wildcard still appears at an operand-start
        // (`SELECT |`, `SELECT a, |`) and for a qualified scope (`t.|`). A bind
        // name slot (`= :|`) is likewise not a wildcard position.
        let wildcard_suggestions = if at_keyword_only_identifier_slot
            || at_keyword_only_slot
            || (qualifier.is_none()
                && (expr_keyword_ctx.follows_operand == Some(true)
                    || expr_keyword_ctx.at_bind_variable_name))
        {
            Vec::new()
        } else {
            Self::collect_clause_wildcard_suggestions(&snapshot.prefix, qualifier, deep_ctx)
        };
        if !wildcard_suggestions.is_empty() {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                wildcard_suggestions,
                true,
            );
        }
        if include_columns && qualifier.is_none() && !restrict_to_relation_columns {
            let derived_columns = Self::collect_derived_columns_for_context(deep_ctx);
            suggestions = if Self::cursor_is_in_query_level_order_by(deep_ctx) {
                Self::merge_suggestions_with_prioritized_derived_columns(
                    suggestions,
                    &snapshot.prefix,
                    derived_columns,
                )
            } else {
                Self::merge_suggestions_with_derived_columns(
                    suggestions,
                    &snapshot.prefix,
                    derived_columns,
                )
            };
        }
        let context_name_suggestions =
            if matches!(context, SqlContext::VariableName | SqlContext::BindValue)
                || restrict_to_relation_columns
                || at_keyword_only_identifier_slot
            {
                Vec::new()
            } else {
                Self::collect_context_name_suggestions(&snapshot.prefix, deep_ctx, context)
            };
        let suggestions = Self::maybe_merge_suggestions_with_context_aliases(
            suggestions,
            context_name_suggestions,
            matches!(context, SqlContext::TableName),
            qualifier.is_some(),
        );
        let mut suggestions = if !local_suggestions.is_empty() {
            Self::prepend_local_symbol_suggestions(suggestions, local_suggestions)
        } else {
            suggestions
        };

        // Offer an FK-based join condition as the top suggestion when filling
        // in a JOIN ... ON clause between two related tables.
        if qualifier.is_none()
            && matches!(deep_ctx.phase, intellisense_context::SqlPhase::JoinCondition)
        {
            let real_tables: Vec<&intellisense_context::ScopedTableRef> = deep_ctx
                .tables_in_scope
                .iter()
                .filter(|table| !table.is_cte)
                .collect();
            if let [lefts @ .., right] = real_tables.as_slice() {
                if !lefts.is_empty() {
                    // Foreign keys are loaded lazily (only here, when a JOIN is
                    // actually being written) to keep ordinary column
                    // completion to a single query per table. A refresh fires
                    // when each load completes, re-running this branch.
                    for table in &real_tables {
                        Self::request_table_foreign_keys(
                            &table.name,
                            intellisense_data,
                            column_sender,
                            connection,
                        );
                    }
                    let condition = {
                        let data = intellisense_data
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        Self::build_auto_join_condition(&data, right, lefts)
                    };
                    if let Some(condition) = condition {
                        if Self::completion_suggestion_matches_prefix(
                            &condition,
                            &snapshot.prefix,
                        ) && !suggestions.iter().any(|s| s == &condition)
                        {
                            suggestions.insert(0, condition);
                        }
                    }
                }
            }
        }

        let should_refresh_when_columns_ready = include_columns && columns_loading;
        if should_refresh_when_columns_ready {
            runtime.set_pending_intellisense(Some(PendingIntellisense {
                cursor_pos: snapshot.cursor_pos,
            }));
        } else {
            runtime.clear_pending_intellisense();
        }

        if suggestions.is_empty() {
            intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            runtime.clear_completion_range();
            return;
        }

        let popup_width = Self::INTELLISENSE_POPUP_WIDTH;
        // Mirror IntellisensePopup's row height (font size + 6, min 20) so the
        // vertical clamp keeps the actual popup on screen.
        let row_h = (crate::ui::configured_ui_font_size() + 6).max(20);
        let popup_height = (suggestions.len().min(10) as i32) * row_h + 10;
        let (popup_x, popup_y) =
            Self::popup_screen_position(editor, snapshot.cursor_pos, popup_width, popup_height);
        struct PopupShowInProgressReset {
            runtime: Arc<IntellisenseRuntimeState>,
        }
        impl Drop for PopupShowInProgressReset {
            fn drop(&mut self) {
                self.runtime
                    .set_popup_transition_state(IntellisensePopupTransitionState::Idle);
            }
        }
        runtime.set_popup_transition_state(IntellisensePopupTransitionState::Showing);
        let _popup_show_reset = PopupShowInProgressReset {
            runtime: runtime.clone(),
        };
        let descriptions = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::build_suggestion_details(
                &data,
                &suggestions,
                &column_tables,
                Some(snapshot.preferred_db_type),
            )
        };
        intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .show_suggestions_with_descriptions(suggestions, descriptions, popup_x, popup_y);
        let completion_start = if snapshot.prefix.is_empty() {
            snapshot.cursor_pos_usize
        } else {
            snapshot.word_start
        };
        runtime.set_completion_range(Some(IntellisenseCompletionRange::new(
            completion_start,
            snapshot.cursor_pos_usize,
        )));
        let mut editor = editor.clone();
        let _ = editor.take_focus();
    }

    /// Column suggestions for a position that is restricted to a concrete
    /// relation scope — an explicit `qualifier.` reference, or a clause whose
    /// grammar only admits a single target relation's columns. When that scope
    /// is empty (an unresolved/invalid qualifier, or a target table not in
    /// scope), the lookup must yield nothing: `IntellisenseData::
    /// get_column_suggestions` treats an empty scope as "every column", and
    /// dumping the whole catalog is never a valid suggestion at a scoped
    /// position. This is the single chokepoint guarding against that fallback,
    /// so every scoped column path stays consistent.
    fn scoped_column_suggestions(
        data: &mut IntellisenseData,
        prefix: &str,
        column_scope: Option<&[String]>,
    ) -> Vec<String> {
        match column_scope {
            Some(scope) if !scope.is_empty() => data.get_column_suggestions(prefix, Some(scope)),
            _ => Vec::new(),
        }
    }

    fn base_suggestions_for_context(
        data: &mut IntellisenseData,
        prefix: &str,
        qualifier: Option<&str>,
        column_scope: Option<&[String]>,
        include_columns: bool,
        context: SqlContext,
        restrict_to_relation_columns: bool,
        db_type: Option<crate::db::DatabaseType>,
        expr_keyword_ctx: ExpressionKeywordContext,
    ) -> Vec<String> {
        if qualifier.is_some() {
            return Self::scoped_column_suggestions(data, prefix, column_scope);
        }

        // A bind-variable name slot (`= :|`, `:b|`) names a free/session-bind
        // identifier, never a column/relation/keyword. Session bind names are
        // supplied separately (the local-symbol path), so the identifier base is
        // empty here.
        if expr_keyword_ctx.at_bind_variable_name {
            return Vec::new();
        }

        if matches!(context, SqlContext::VariableName | SqlContext::BindValue) {
            return Vec::new();
        }

        if matches!(context, SqlContext::TableName) {
            return data.get_relation_suggestions(prefix);
        }

        if restrict_to_relation_columns {
            return Self::scoped_column_suggestions(data, prefix, column_scope);
        }

        let prefer_columns = matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll);
        let mut suggestions = data.get_suggestions_for_db(
            prefix,
            include_columns,
            column_scope,
            false,
            prefer_columns,
            db_type,
        );
        // The base catalog mixes columns, relations, objects and functions (kept
        // as-is) with a *flat, prefix-only* dump of the entire keyword list. That
        // dump is the sole source of keyword noise in a value/column expression —
        // it offers clause/statement/DDL keywords (`FROM`/`CREATE`/`TABLESPACE`),
        // construct-scoped keywords outside their construct (`END` with no `CASE`,
        // `PRECEDING` with no window), and operators in the wrong slot (`AND` at an
        // operand-start). Instead of blocklisting the bad ones (open-ended, risks
        // hiding a valid keyword) we *allowlist* the keywords that are genuinely
        // grammatical at the cursor: a keyword survives only when it can appear at
        // this expression position. Every keyword valid only at a dedicated slot
        // (data type, EXTRACT field, INTERVAL unit, JSON `RETURNING`, window frame
        // bound, MERGE `WHEN`, …) is already re-supplied by the grammar-aware
        // `collect_expected_keyword_suggestions` merge, so dropping it from this
        // flat dump cannot hide it where it belongs. Columns, relations, objects
        // and functions are never touched, and a column named like a keyword is
        // preserved.
        if prefer_columns {
            suggestions.retain(|suggestion| {
                Self::expression_suggestion_is_relevant(
                    data,
                    suggestion,
                    column_scope,
                    db_type,
                    expr_keyword_ctx,
                )
            });
        }
        // A value expression (`v := |`, `proc(p => |)`, an `IF`/`WHILE` control
        // condition, a routine call argument `f(|)`) admits a variable/function/
        // literal but never a bare table/view/synonym. Only the General context
        // dumps the whole relation catalog into the base (a column context offers
        // scoped columns instead — some of which may be named like a table — so
        // the filter is restricted to `!prefer_columns` to avoid dropping such a
        // column). The relation names are pure noise here, while a variable,
        // function, package or literal still completes; a name that is also a
        // function/package is kept.
        if !prefer_columns && expr_keyword_ctx.in_plsql_value_expression {
            suggestions.retain(|suggestion| {
                !data.is_known_relation(suggestion) || data.is_language_function(&suggestion.to_ascii_uppercase(), db_type)
            });
        }
        // The same flat-base keyword noise (`WHERE`/`WHILE`/`CREATE`/a stray
        // `WHEN`/`THEN` after a closed `END CASE`, …) also leaks into a General-
        // context PL/SQL value expression, which the `prefer_columns` allowlist
        // above does not cover. Apply it here too, but only at a genuine value-
        // *operand* position (`v := |`, `IF v > |`, `RETURN |`) — never at a block
        // statement start, where `IF`/`LOOP`/`RETURN`/… are the valid keywords.
        if !prefer_columns && expr_keyword_ctx.at_plsql_value_operand {
            suggestions.retain(|suggestion| {
                Self::expression_suggestion_is_relevant(
                    data,
                    suggestion,
                    column_scope,
                    db_type,
                    expr_keyword_ctx,
                )
            });
        }
        // At a statement start the only keyword family that is grammatical is the
        // statement verbs themselves (a top-level `SELECT`/`CREATE`/…, or a PL/SQL
        // block `IF`/`LOOP`/`RETURN`/… and its construct continuations). The flat
        // base dump otherwise offers every prefix-matched keyword — clause words,
        // type names, modifiers, function keywords — none of which can begin a
        // statement. Object/identifier material is filtered the same way: a SQL
        // top-level statement never starts with an identifier or a function call,
        // and a PL/SQL block statement admits only a bare procedure/package call
        // (never a function — its result cannot stand as a statement — a sequence,
        // or a table/view).
        if !prefer_columns {
            if let Some(statement_start) = expr_keyword_ctx.statement_start {
                suggestions.retain(|suggestion| {
                    Self::suggestion_begins_statement(data, suggestion, statement_start, db_type)
                });
            }
        }
        suggestions
    }

    /// Whether a single base-catalog entry can stand where the cursor begins a
    /// statement. Keyword entries defer to `keyword_begins_statement`; non-keyword
    /// entries (identifiers, `NAME()` functions) are admitted only as a PL/SQL
    /// block's bare procedure/package call.
    fn suggestion_begins_statement(
        data: &IntellisenseData,
        suggestion: &str,
        ctx: StatementStartContext,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let upper = suggestion.to_ascii_uppercase();
        if data.is_language_keyword(&upper, db_type) {
            return Self::keyword_begins_statement(&upper, ctx);
        }
        match ctx {
            StatementStartContext::TopLevel => false,
            StatementStartContext::Plsql(policy) => {
                policy.allow_statements
                    && matches!(
                        data.suggestion_type_label(suggestion, db_type),
                        Some("PROCEDURE" | "PACKAGE")
                    )
            }
        }
    }

    /// Whether `upper` (a reserved keyword) can stand at the statement-start
    /// position described by `ctx`. Top-level statements admit the SQL/PLSQL-block
    /// statement verbs; a PL/SQL block additionally admits the procedural
    /// statement keywords plus the continuations of the construct enclosing the
    /// cursor.
    fn keyword_begins_statement(upper: &str, ctx: StatementStartContext) -> bool {
        // SQL statement verbs — the keywords that open a top-level statement.
        const SQL_STATEMENT_KEYWORDS: &[&str] = &[
            "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "WITH", "CREATE", "ALTER", "DROP",
            "TRUNCATE", "GRANT", "REVOKE", "COMMENT", "RENAME", "CALL", "EXPLAIN", "ANALYZE",
            "AUDIT", "NOAUDIT", "LOCK", "SET", "COMMIT", "ROLLBACK", "SAVEPOINT", "BEGIN",
            "DECLARE", "PURGE", "FLASHBACK", "EXEC", "EXECUTE",
        ];
        // Procedural statement keywords that open a PL/SQL block statement.
        const PLSQL_STATEMENT_KEYWORDS: &[&str] = &[
            "IF", "CASE", "LOOP", "WHILE", "FOR", "FORALL", "GOTO", "NULL", "RETURN", "RAISE",
            "BEGIN", "DECLARE", "OPEN", "CLOSE", "FETCH", "EXECUTE", "COMMIT", "ROLLBACK",
            "SAVEPOINT", "SET", "LOCK", "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "WITH",
            "PIPE",
        ];

        match ctx {
            StatementStartContext::TopLevel => SQL_STATEMENT_KEYWORDS.contains(&upper),
            StatementStartContext::Plsql(policy) => {
                if policy.allow_statements && PLSQL_STATEMENT_KEYWORDS.contains(&upper) {
                    return true;
                }
                match upper {
                    "EXIT" | "CONTINUE" => policy.allow_exit_continue,
                    "END" => policy.allow_end,
                    "WHEN" => policy.allow_when,
                    "ELSIF" => policy.allow_elsif,
                    "ELSE" => policy.allow_else,
                    "EXCEPTION" => policy.allow_exception,
                    _ => false,
                }
            }
        }
    }

    /// Decide whether a single base-catalog entry is grammatical at the current
    /// value/column expression position. Operand material (columns, relations,
    /// objects, rendered functions, value-producing function keywords) passes
    /// only where an operand is expected; keywords pass only when the position's
    /// allowlist admits them. A real column named like a keyword is preserved.
    fn expression_suggestion_is_relevant(
        data: &mut IntellisenseData,
        suggestion: &str,
        column_scope: Option<&[String]>,
        db_type: Option<crate::db::DatabaseType>,
        expr_keyword_ctx: ExpressionKeywordContext,
    ) -> bool {
        // A rendered function (`NAME()`), a column/relation/object identifier, and
        // a value-producing function keyword are all *operand material*: they are
        // grammatical only where a new operand is expected. Right after a complete
        // operand the only valid continuations are operators (handled below as
        // keywords); a bare identifier there would be an implicit alias, never an
        // existing name — so operand material is dropped at that slot. When the
        // position is ambiguous (`follows_operand == None`) it is kept, so a valid
        // completion is never hidden.
        let after_operand = expr_keyword_ctx.follows_operand == Some(true);
        if suggestion.ends_with("()") {
            return !after_operand;
        }
        let upper = suggestion.to_ascii_uppercase();
        if !data.is_language_keyword(&upper, db_type) {
            // Column / relation / object identifier: operand material only.
            return !after_operand;
        }
        if Self::keyword_is_grammatical_in_expression(&upper, expr_keyword_ctx, db_type) {
            return true;
        }
        if !after_operand && data.is_language_function(&upper, db_type) {
            return true;
        }
        // Preserve a real column that happens to be named like a keyword (operand
        // material, so only where an operand is expected).
        !after_operand
            && column_scope.is_some_and(|scope| {
                data.get_column_suggestions(&upper, Some(scope))
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(&upper))
            })
    }

    /// The allowlist core: is `upper` (a reserved keyword) grammatical at the
    /// current expression position? Splits keywords into operand-start vs
    /// after-operand families and gates the construct-scoped ones (CASE body,
    /// ORDER BY direction) on the enclosing construct.
    fn keyword_is_grammatical_in_expression(
        upper: &str,
        ctx: ExpressionKeywordContext,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        // Keywords valid where a *new operand/expression* is expected. Excluded
        // and gated separately below: the set-quantifiers `ALL`/`DISTINCT`/
        // `UNIQUE`/`DISTINCTROW` (list/aggregate/set anchor only), quantified
        // comparison keywords `ALL`/`ANY`/`SOME` (`x = ANY (...)`), the
        // Oracle `CURSOR(...)` expression, MySQL full-text `MATCH` call, MySQL
        // trigger pseudo rows `NEW`/`OLD`, the hierarchical pseudo-columns/
        // operators (CONNECT BY query only), and `DEFAULT` (DML value position
        // only).
        // `ROWNUM`/`ROWID` stay because they are valid pseudo-columns in any query.
        const OPERAND_START_KEYWORDS: &[&str] = &[
            "CASE", "CAST", "EXISTS", "NOT", "NULL", "TRUE", "FALSE", "UNKNOWN", "INTERVAL",
            "DATE", "TIMESTAMP", "TIME", "ROWNUM", "ROWID", "MULTISET", "BINARY",
        ];
        // Set-quantifiers: grammatical only at a select-list/aggregate anchor —
        // `SELECT ALL`, `UNION ALL`, `COUNT(DISTINCT x)`. They always sit at such
        // an anchor, so gating on `follows_quantifier_anchor` removes the noise of
        // offering them as a general operand without hiding a valid one.
        const SELECT_QUANTIFIER_KEYWORDS: &[&str] =
            &["ALL", "DISTINCT", "UNIQUE", "DISTINCTROW"];
        const QUANTIFIED_COMPARISON_KEYWORDS: &[&str] = &["ALL", "ANY", "SOME"];
        // Hierarchical pseudo-columns/operators: grammatical only in a query that
        // has a `CONNECT BY` clause, where an operand is expected.
        const HIERARCHICAL_KEYWORDS: &[&str] = &[
            "LEVEL", "PRIOR", "CONNECT_BY_ROOT", "CONNECT_BY_ISCYCLE", "CONNECT_BY_ISLEAF",
        ];
        // Operators/continuations valid *after a complete operand*. Continuations
        // that require a *specific* preceding operand are excluded here and gated
        // separately below: `OVER`/`KEEP`/`WITHIN` (closed call), `AGAINST`
        // (`MATCH` call),
        // `ESCAPE` (`LIKE` pattern), `SEPARATOR` (`GROUP_CONCAT` arguments),
        // `SOUNDS` (`SOUNDS LIKE`), and the operand-type operators `AT`
        // (datetime), `COLLATE` (character), `MEMBER`/`SUBMULTISET`/`MULTISET`
        // (collection). Every other operator is grammatical after any operand.
        const AFTER_OPERAND_KEYWORDS: &[&str] = &[
            "AND", "OR", "NOT", "IN", "IS", "LIKE", "LIKE2", "LIKE4", "LIKEC", "BETWEEN",
            "DIV", "MOD", "XOR", "REGEXP", "RLIKE",
        ];
        // Analytic/aggregate continuations: grammatical only immediately after a
        // closed call `…)` — `SUM(x) OVER`, `MAX(x) KEEP (...)`,
        // `LISTAGG(..) WITHIN GROUP (...)`. They always follow a `)`, so gating on
        // `follows_call` removes the noise of offering them after a column/literal
        // without ever hiding a valid completion.
        const CALL_CONTINUATION_KEYWORDS: &[&str] = &["OVER", "KEEP", "WITHIN"];
        // CASE body: only inside an unclosed CASE.
        if ctx.inside_case && Self::is_case_clause_keyword(upper) {
            return true;
        }
        // ORDER BY direction / null-ordering continuations follow a sort operand.
        if ctx.in_order_by
            && ctx.follows_operand != Some(false)
            && matches!(upper, "ASC" | "DESC" | "NULLS")
        {
            return true;
        }

        // Call continuations: only right after a closed call `…)`.
        if ctx.follows_call && CALL_CONTINUATION_KEYWORDS.contains(&upper) {
            return true;
        }
        if upper == "AGAINST" {
            return ctx.follows_match_call
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        // `ESCAPE`: only right after a `LIKE` pattern. It always follows one, so
        // gating on `follows_like_pattern` removes the noise of offering it after
        // a plain operand without ever hiding a valid completion.
        if ctx.follows_like_pattern && upper == "ESCAPE" {
            return true;
        }
        if upper == "SEPARATOR" {
            return ctx.follows_operand == Some(true)
                && ctx.inside_group_concat_arguments_before_separator
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "SOUNDS" {
            return ctx.follows_operand == Some(true)
                && matches!(
                    ctx.prev_operand_type,
                    PrecedingOperandType::Character | PrecedingOperandType::Unknown
                )
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "LIKE" && ctx.follows_sounds_operator {
            return crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "MATCH" {
            return ctx.follows_operand != Some(true)
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "CURSOR" {
            return ctx.follows_operand != Some(true)
                && !crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "NEW" {
            return ctx.follows_operand != Some(true)
                && ctx.mysql_trigger_allows_new
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        if upper == "OLD" {
            return ctx.follows_operand != Some(true)
                && ctx.mysql_trigger_allows_old
                && crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        // Set-quantifiers: only at a select-list/aggregate anchor.
        if ctx.follows_quantifier_anchor && SELECT_QUANTIFIER_KEYWORDS.contains(&upper) {
            return true;
        }
        // Quantified comparison keywords: only after a comparison operator.
        if ctx.follows_quantified_comparison_operator
            && QUANTIFIED_COMPARISON_KEYWORDS.contains(&upper)
        {
            return true;
        }
        // Hierarchical pseudo-columns/operators: only in a CONNECT BY query, where
        // an operand is expected (never right after a complete operand).
        if ctx.has_connect_by && HIERARCHICAL_KEYWORDS.contains(&upper) {
            return ctx.follows_operand != Some(true);
        }
        // `DEFAULT` value keyword: only in a DML value position, where an operand
        // is expected (`VALUES (… , DEFAULT)`, `SET col = DEFAULT`).
        if ctx.in_dml_value_position && upper == "DEFAULT" {
            return ctx.follows_operand != Some(true);
        }
        // Operand-type postfix operators: grammatical only *after* an operand, and
        // only of the matching type. `AT` (`… AT TIME ZONE`) needs a datetime,
        // `COLLATE` a character value. The type is used to *exclude* a provable
        // mismatch (a number/date before `COLLATE`, a string before `AT`); when it
        // cannot be determined the operator is kept, so a valid completion after an
        // operand whose type we could not resolve is never hidden.
        if upper == "AT" {
            return ctx.follows_operand == Some(true)
                && matches!(
                    ctx.prev_operand_type,
                    PrecedingOperandType::Datetime | PrecedingOperandType::Unknown
                );
        }
        if upper == "COLLATE" {
            return ctx.follows_operand == Some(true)
                && matches!(
                    ctx.prev_operand_type,
                    PrecedingOperandType::Character | PrecedingOperandType::Unknown
                );
        }
        // The collection operators (`MEMBER`/`SUBMULTISET`, and `MULTISET` used as
        // a set-operator after an operand) require a nested-table operand. A
        // collection type is never inferable from column metadata, so — unlike the
        // datetime/character operators above — keeping them on an unknown operand
        // would mean dumping them after *every* operand. They are therefore only
        // offered when the operand is provably a collection. (`MULTISET` as a
        // collection *constructor*, where an operand is expected, falls through to
        // the operand-start list below.)
        if matches!(upper, "MEMBER" | "SUBMULTISET") {
            return ctx.prev_operand_type == PrecedingOperandType::Collection;
        }
        if upper == "MULTISET" && ctx.follows_operand == Some(true) {
            return ctx.prev_operand_type == PrecedingOperandType::Collection;
        }

        let allow_start = ctx.follows_operand != Some(true);
        let allow_after = ctx.follows_operand != Some(false);

        if allow_after && AFTER_OPERAND_KEYWORDS.contains(&upper) {
            return true;
        }
        if allow_start && OPERAND_START_KEYWORDS.contains(&upper) {
            return true;
        }
        false
    }

    /// The keywords that only occur inside a `CASE` expression's body.
    fn is_case_clause_keyword(word: &str) -> bool {
        matches!(
            word.to_ascii_uppercase().as_str(),
            "WHEN" | "THEN" | "ELSE" | "ELSIF" | "ELSEIF" | "END"
        )
    }

    /// True when the cursor sits inside a PL/SQL *executable* block body — after a
    /// `BEGIN`, or inside an `IF`/`LOOP`/`CASE` opened within one (including its
    /// condition header, `EXCEPTION` handlers, and `RETURN`/`RAISE`/assignment
    /// statements). In an executable body a bare table/view/synonym is never a
    /// valid operand — code references variables, functions, packages and
    /// types; the only relations live in *embedded SQL* statements, which carry
    /// their own `FromClause`/`TableName` phase and are handled elsewhere. The
    /// *declaration* section is deliberately excluded: a `DECLARE`/`IS` type slot
    /// (`v emp.empno%TYPE`, `v emp%ROWTYPE`) legitimately names a relation, so the
    /// frame stack distinguishes it (its top frame is the pending declaration, not
    /// an executable block).
    ///
    /// Mirrors the formatter's `WithPlsqlBodyFrame` model — push on
    /// `BEGIN`/`IF`/`LOOP`/`CASE` (a `DECLARE` opens a *pending* block that
    /// `BEGIN` promotes), pop on `END` — reusing the shared PL/SQL control-keyword
    /// vocabulary rather than running the formatter's output state machine. Pushes
    /// and pops balance 1:1 regardless of `END`/`END IF|LOOP|CASE` qualifiers, so
    /// the innermost open frame is an exact "executable vs declaring" signal.
    fn cursor_in_plsql_executable_block(tokens: &[SqlToken], end: usize) -> bool {
        // Frame kinds are tracked separately because `CASE` is shared between a
        // SQL `CASE` *expression* and a PL/SQL `CASE` *statement*: a `CASE` that is
        // not inside a block is just a SQL value expression and must not be read as
        // PL/SQL code. `IF`/`LOOP` are PL/SQL-only, so they alone mark executable
        // code; `BEGIN` (a `DECLARE` promoted by its `BEGIN`) is the block marker.
        #[derive(Clone, Copy, PartialEq)]
        enum Frame {
            PendingDeclare,
            Begin,
            Control, // IF / LOOP — PL/SQL only
            Case,    // shared SQL/PL-SQL — not on its own a PL/SQL signal
        }
        let mut stack: Vec<Frame> = Vec::new();
        for token in tokens.get(..end.min(tokens.len())).unwrap_or(tokens) {
            let SqlToken::Word(word) = token else {
                continue;
            };
            match word.to_ascii_uppercase().as_str() {
                "DECLARE" => stack.push(Frame::PendingDeclare),
                "BEGIN" => {
                    if matches!(stack.last(), Some(Frame::PendingDeclare)) {
                        if let Some(last) = stack.last_mut() {
                            *last = Frame::Begin;
                        }
                    } else {
                        stack.push(Frame::Begin);
                    }
                }
                "IF" | "LOOP" => stack.push(Frame::Control),
                "CASE" => stack.push(Frame::Case),
                "END" => {
                    stack.pop();
                }
                _ => {}
            }
        }
        // Executable when an actual block is open, or the innermost construct is a
        // PL/SQL-only `IF`/`LOOP` (a bare `CASE` without an enclosing block is a SQL
        // expression, never PL/SQL code).
        stack.iter().any(|frame| matches!(frame, Frame::Begin))
            || matches!(stack.last(), Some(Frame::Control))
    }

    /// True when the cursor sits inside a routine *call's* argument list —
    /// `proc(|)`, `dbms_output.put_line(|)`, `pkg.fn(a, |)`. An argument is a
    /// value expression, so a variable/function/literal completes but a bare
    /// table/view/synonym never does. Detection finds the innermost still-open
    /// `(`, requires it to be introduced by a routine name (a non-keyword
    /// identifier immediately before it) and to not open a subquery (`fn(SELECT
    /// …)` / `CURSOR(SELECT …)` keep relation completion for their inner query —
    /// the same `is_query_expression_start` test the wildcard nesting uses).
    fn cursor_in_call_argument_list(tokens: &[SqlToken], end: usize) -> bool {
        let mut depth = 0i32;
        let mut open_idx = None;
        for idx in (0..end.min(tokens.len())).rev() {
            match &tokens[idx] {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    if depth == 0 {
                        open_idx = Some(idx);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let Some(open_idx) = open_idx else {
            return false;
        };
        if intellisense_context::is_query_expression_start(tokens, open_idx + 1) {
            return false;
        }
        // The token immediately before `(` must be a routine name: a plain
        // identifier, not a keyword (which would mark an `IN (…)` / `VALUES (…)` /
        // expression group rather than a call).
        matches!(
            tokens.get(..open_idx)
                .unwrap_or(tokens)
                .iter()
                .rev()
                .find(|token| !matches!(token, SqlToken::Comment(_))),
            Some(SqlToken::Word(word)) if !Self::token_is_language_keyword(&word.to_ascii_uppercase())
        )
    }

    /// Gather the cursor-position facts that drive the expression-keyword
    /// allowlist for the current query. `data`/`column_scope` are consulted to
    /// infer the preceding operand's type for the type-gated postfix operators.
    fn expression_keyword_context(
        deep_ctx: &intellisense_context::CursorContext,
        data: &IntellisenseData,
        column_scope: &[String],
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> ExpressionKeywordContext {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let inside_case = Self::cursor_is_inside_unclosed_case(tokens, cursor_len);
        let in_order_by = matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause
        );
        // Drop the identifier chain at the cursor only when the user is actually
        // typing one (a non-empty prefix). With an empty prefix the cursor sits
        // after a completed token, so excluding it would point `end` at the token
        // *before* the finished operand and misread a complete operand as an
        // operand-start — leaking columns/`*` after `c = 'x' `/`SELECT empno `.
        // Mirrors the flag `collect_expected_keyword_suggestions` already passes.
        let end =
            Self::expected_suggestion_context_end(tokens, cursor_len, exclude_current_identifier_chain);
        let follows_operand = Self::cursor_follows_complete_operand(tokens, end);
        // The analytic/aggregate continuations (`OVER`/`KEEP`/`WITHIN`)
        // follow the `)` of a *function call*, not a grouping/predicate paren
        // (`(a + b) `) or a scalar subquery (`(SELECT … ) `). `call_name_before_close`
        // returns the name introducing the closed paren — present only for a call,
        // and a real function (`SUM`/`LISTAGG`/…) rather than a clause keyword like
        // `SELECT` that can also precede a `(`.
        let closed_call_name = {
            let before = Self::meaningful_tokens_before(tokens, end);
            if matches!(before.last(), Some(SqlToken::Symbol(sym)) if sym == ")") {
                Self::call_name_before_close(&before)
            } else {
                None
            }
        };
        let follows_call = closed_call_name
            .as_deref()
            .is_some_and(|name| Self::name_introduces_call(data, name, db_type));
        let follows_match_call = closed_call_name
            .as_deref()
            .is_some_and(|name| name == "MATCH");
        let follows_sounds_operator = Self::cursor_follows_sounds_operator(tokens, end);
        let (mysql_trigger_allows_new, mysql_trigger_allows_old) = {
            let full_tokens = deep_ctx.statement_tokens.as_ref();
            let full_end = Self::expected_suggestion_context_end(
                full_tokens,
                deep_ctx.cursor_token_len,
                exclude_current_identifier_chain,
            );
            Self::mysql_trigger_pseudo_row_policy(full_tokens, full_end)
        };
        let follows_like_pattern = Self::cursor_follows_like_pattern(tokens, end);
        let inside_group_concat_arguments_before_separator =
            Self::cursor_in_group_concat_arguments_before_separator(tokens, end);
        let follows_quantifier_anchor = {
            let before = Self::meaningful_tokens_before(tokens, end);
            match before.last() {
                // A set-quantifier (`DISTINCT`/`UNIQUE`/`DISTINCTROW`) follows `(`
                // only when the paren opens an *aggregate function call*
                // (`COUNT(DISTINCT x)`), never a grouping/predicate paren
                // (`(a + b)`, `WHERE (x`). The call paren is the one introduced by
                // a function-name word; a grouping paren is preceded by an
                // operator/keyword/`(`/`,` (or nothing), so it is not an anchor.
                Some(SqlToken::Symbol(sym)) if sym == "(" => matches!(
                    before.len().checked_sub(2).and_then(|i| before.get(i)),
                    Some(SqlToken::Word(word))
                        if Self::name_introduces_call(data, &word.to_ascii_uppercase(), db_type)
                ),
                Some(SqlToken::Word(word)) => matches!(
                    word.to_ascii_uppercase().as_str(),
                    "SELECT" | "UNION" | "EXCEPT" | "INTERSECT" | "MINUS"
                ),
                _ => false,
            }
        };
        let follows_quantified_comparison_operator =
            Self::cursor_follows_quantified_comparison_operator(tokens, end);
        let has_connect_by = Self::cursor_query_has_connect_by(tokens);
        let in_dml_value_position = matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::SetClause
                | intellisense_context::SqlPhase::ValuesClause
        );
        let prev_operand_type =
            Self::preceding_operand_type(tokens, end, data, column_scope);
        let in_plsql_value_expression = matches!(
            Self::meaningful_tokens_before(tokens, end).last(),
            Some(SqlToken::Symbol(sym)) if sym == ":=" || sym == "=>"
        ) || Self::cursor_in_plsql_executable_block(tokens, end)
            || Self::cursor_in_call_argument_list(tokens, end);
        // A value-operand position is one where the previous token can only
        // introduce an operand — never begin a statement — so the expression
        // keyword allowlist is safe to run without hiding the statement keywords
        // valid at a block statement start (`BEGIN |`, `; |`, `THEN |`).
        let at_plsql_value_operand = in_plsql_value_expression
            && match Self::meaningful_tokens_before(tokens, end).last() {
                Some(SqlToken::Symbol(sym)) => matches!(
                    sym.as_str(),
                    ":=" | "=>" | "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "^="
                        | "+" | "-" | "*" | "/" | "||"
                ),
                Some(SqlToken::Word(word)) => matches!(
                    word.to_ascii_uppercase().as_str(),
                    "AND" | "OR" | "NOT" | "IN" | "LIKE" | "BETWEEN" | "MOD" | "XOR"
                        | "DIV" | "RETURN" | "RAISE" | "IF" | "ELSIF" | "WHILE"
                ),
                _ => false,
            };
        let at_bind_variable_name = matches!(
            Self::meaningful_tokens_before(tokens, end).last(),
            Some(SqlToken::Symbol(sym)) if sym == ":"
        );
        // A statement start only exists in the neutral General context — the
        // statement-head position. A clause phase (a select list, a `WHERE`
        // predicate, a set-operation branch) is never a statement start. Boundary
        // detection runs on the *full* statement tokens, not the query-scoped
        // slice: a narrowed set-operation branch begins right after `UNION`, so its
        // slice would look like a fresh statement (no preceding token) even though
        // the real previous token (`UNION`) marks a query continuation, not a
        // statement boundary.
        let statement_start =
            if matches!(sql_context_for_phase(deep_ctx.phase), SqlContext::General) {
                let full_tokens = deep_ctx.statement_tokens.as_ref();
                let full_end = Self::expected_suggestion_context_end(
                    full_tokens,
                    deep_ctx.cursor_token_len,
                    exclude_current_identifier_chain,
                );
                Self::cursor_statement_start_context(full_tokens, full_end)
            } else {
                None
            };
        ExpressionKeywordContext {
            inside_case,
            in_order_by,
            follows_operand,
            follows_call,
            follows_match_call,
            follows_sounds_operator,
            mysql_trigger_allows_new,
            mysql_trigger_allows_old,
            follows_like_pattern,
            inside_group_concat_arguments_before_separator,
            follows_quantifier_anchor,
            follows_quantified_comparison_operator,
            has_connect_by,
            in_dml_value_position,
            prev_operand_type,
            in_plsql_value_expression,
            at_plsql_value_operand,
            at_bind_variable_name,
            statement_start,
        }
    }

    /// Classify whether the cursor begins a fresh statement, and resolve which
    /// keyword family is grammatical there. A top-level statement start is a
    /// position after nothing or `;`. Inside a PL/SQL executable block the
    /// position is resolved by a construct scan (`plsql_keyword_policy`) that
    /// tracks the enclosing block/`IF`/`CASE`/`LOOP` and its state, so the flat
    /// keyword dump is filtered to exactly the procedural keywords and the
    /// construct continuations valid at the cursor.
    fn cursor_statement_start_context(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<StatementStartContext> {
        // A routine-call argument list is a value position, never a statement
        // start (`proc(|)`), even though its previous token is a `(` separator.
        if Self::cursor_in_call_argument_list(tokens, end) {
            return None;
        }

        if Self::cursor_in_plsql_executable_block(tokens, end) {
            return Self::plsql_keyword_policy(tokens, end).map(StatementStartContext::Plsql);
        }

        // Top level: a statement begins only after nothing or a `;` terminator.
        let prev = Self::meaningful_tokens_before(tokens, end).last().copied();
        match prev {
            None => Some(StatementStartContext::TopLevel),
            Some(SqlToken::Symbol(sym)) if sym == ";" => Some(StatementStartContext::TopLevel),
            _ => None,
        }
    }

    /// Resolve the PL/SQL keyword policy at the cursor by scanning the construct
    /// stack to it. Returns `None` when the cursor is *not* at a statement or
    /// construct-continuation position (a condition/selector operand, a value
    /// expression), where the operand allowlist governs instead. `END <IF|LOOP|
    /// CASE>` closes one construct and its qualifier keyword is skipped.
    fn plsql_keyword_policy(tokens: &[SqlToken], end: usize) -> Option<PlsqlKeywordPolicy> {
        #[derive(Clone, Copy)]
        enum Frame {
            PendingDeclare,
            // A block body; `in_exception` once its `EXCEPTION` handler section
            // has begun.
            Block { in_exception: bool },
            // `awaiting_then` between `IF`/`ELSIF` and `THEN` (the condition);
            // `in_else` once the `ELSE` arm has begun.
            If { awaiting_then: bool, in_else: bool },
            Loop,
            // A `CASE`: `is_statement` distinguishes a PL/SQL `CASE` statement
            // (its arms are statements) from a `CASE` value expression (arms are
            // values). `past_selector` once the first `WHEN` is seen;
            // `awaiting_then` between a `WHEN` and its `THEN`; `in_else` after the
            // `ELSE` arm begins.
            Case {
                is_statement: bool,
                past_selector: bool,
                awaiting_then: bool,
                in_else: bool,
            },
        }

        let toks = tokens.get(..end.min(tokens.len())).unwrap_or(tokens);
        let mut stack: Vec<Frame> = Vec::new();
        let mut skip_end_qualifier = false;
        // Whether the previous significant token closes a statement, so a `CASE`
        // that follows it opens a statement (not a value expression).
        let mut prev_is_stmt_boundary = true;
        let mut last_word_upper: Option<String> = None;

        for token in toks {
            let upper = match token {
                SqlToken::Word(word) => word.to_ascii_uppercase(),
                SqlToken::Comment(_) => continue,
                SqlToken::Symbol(sym) => {
                    prev_is_stmt_boundary = sym == ";";
                    last_word_upper = None;
                    continue;
                }
                // A literal/other token is an operand — it neither opens a
                // construct nor marks a statement boundary.
                _ => {
                    prev_is_stmt_boundary = false;
                    last_word_upper = None;
                    continue;
                }
            };

            if skip_end_qualifier {
                skip_end_qualifier = false;
                if matches!(upper.as_str(), "IF" | "LOOP" | "CASE") {
                    last_word_upper = Some(upper);
                    prev_is_stmt_boundary = false;
                    continue;
                }
            }

            match upper.as_str() {
                "DECLARE" => stack.push(Frame::PendingDeclare),
                "BEGIN" => {
                    if matches!(stack.last(), Some(Frame::PendingDeclare)) {
                        if let Some(last) = stack.last_mut() {
                            *last = Frame::Block {
                                in_exception: false,
                            };
                        }
                    } else {
                        stack.push(Frame::Block { in_exception: false });
                    }
                }
                "IF" => stack.push(Frame::If {
                    awaiting_then: true,
                    in_else: false,
                }),
                "ELSIF" => {
                    if let Some(Frame::If { awaiting_then, .. }) = stack.last_mut() {
                        *awaiting_then = true;
                    }
                }
                "ELSE" => {
                    if let Some(Frame::If { in_else, awaiting_then })
                    | Some(Frame::Case { in_else, awaiting_then, .. }) = stack.last_mut()
                    {
                        *in_else = true;
                        *awaiting_then = false;
                    }
                }
                "THEN" => match stack.last_mut() {
                    Some(Frame::If { awaiting_then, .. })
                    | Some(Frame::Case { awaiting_then, .. }) => *awaiting_then = false,
                    _ => {}
                },
                "LOOP" => stack.push(Frame::Loop),
                "CASE" => stack.push(Frame::Case {
                    is_statement: prev_is_stmt_boundary,
                    past_selector: false,
                    awaiting_then: false,
                    in_else: false,
                }),
                "WHEN" => match stack.last_mut() {
                    Some(Frame::Case { past_selector, awaiting_then, .. }) => {
                        *past_selector = true;
                        *awaiting_then = true;
                    }
                    // An exception handler `WHEN` also awaits its `THEN`.
                    Some(Frame::Block { .. }) => {}
                    _ => {}
                },
                "EXCEPTION" => {
                    if let Some(Frame::Block { in_exception }) = stack.last_mut() {
                        *in_exception = true;
                    }
                }
                "END" => {
                    stack.pop();
                    skip_end_qualifier = true;
                }
                _ => {}
            }

            prev_is_stmt_boundary =
                matches!(upper.as_str(), "THEN" | "LOOP" | "ELSE" | "BEGIN" | "EXCEPTION");
            last_word_upper = Some(upper);
        }

        let any_loop = stack.iter().any(|frame| matches!(frame, Frame::Loop));
        let prev_word = last_word_upper.as_deref();
        // The cursor is at a fresh statement/continuation position when the last
        // significant token closes a statement (`;`) or opens a body (`THEN`/
        // `LOOP`/`ELSE`/`BEGIN`/`EXCEPTION`). An operator/operand token (`:=`, `+`,
        // a finished expression) is not a boundary — that is an operand position
        // governed by the expression allowlist.
        let prev_is_boundary = prev_is_stmt_boundary;

        let policy_with = |f: fn(&mut PlsqlKeywordPolicy)| {
            let mut p = PlsqlKeywordPolicy::default();
            f(&mut p);
            p
        };

        match stack.last().copied() {
            None => None,
            Some(Frame::PendingDeclare) => None,
            Some(Frame::Block { in_exception }) => {
                // Directly after `EXCEPTION`, only `WHEN` opens the first handler.
                if prev_word == Some("EXCEPTION") {
                    return Some(policy_with(|p| p.allow_when = true));
                }
                if !prev_is_boundary {
                    return None;
                }
                Some(PlsqlKeywordPolicy {
                    allow_statements: true,
                    allow_when: in_exception,
                    allow_end: true,
                    allow_exception: !in_exception,
                    allow_exit_continue: any_loop,
                    ..Default::default()
                })
            }
            Some(Frame::If { awaiting_then, in_else }) => {
                if awaiting_then || !prev_is_boundary {
                    return None;
                }
                Some(PlsqlKeywordPolicy {
                    allow_statements: true,
                    allow_elsif: !in_else,
                    allow_else: !in_else,
                    allow_end: true,
                    allow_exit_continue: any_loop,
                    ..Default::default()
                })
            }
            Some(Frame::Loop) => {
                if !prev_is_boundary {
                    return None;
                }
                Some(PlsqlKeywordPolicy {
                    allow_statements: true,
                    allow_end: true,
                    allow_exit_continue: true,
                    ..Default::default()
                })
            }
            Some(Frame::Case {
                is_statement,
                past_selector,
                awaiting_then,
                in_else,
            }) => {
                if awaiting_then {
                    return None;
                }
                if !past_selector {
                    // The selector slot before the first `WHEN`: only `WHEN`.
                    return Some(policy_with(|p| p.allow_when = true));
                }
                // In an arm body. A statement `CASE` admits statements; a value
                // `CASE` admits only the `WHEN`/`ELSE`/`END` continuations after
                // its arm value. The continuations need the arm value/statement to
                // be complete: a statement boundary, or (value `CASE`) a finished
                // operand.
                if is_statement {
                    if !prev_is_boundary {
                        return None;
                    }
                    Some(PlsqlKeywordPolicy {
                        allow_statements: true,
                        allow_when: !in_else,
                        allow_else: !in_else,
                        allow_end: true,
                        allow_exit_continue: any_loop,
                        ..Default::default()
                    })
                } else {
                    Some(PlsqlKeywordPolicy {
                        allow_when: !in_else,
                        allow_else: !in_else,
                        allow_end: true,
                        ..Default::default()
                    })
                }
            }
        }
    }

    /// Whether the current query level contains a `CONNECT BY` clause, which makes
    /// the hierarchical pseudo-columns/operators grammatical. A bare `CONNECT`
    /// word at the top paren level is the defining marker.
    fn cursor_query_has_connect_by(tokens: &[SqlToken]) -> bool {
        // `CONNECT BY` is a clause of *this* query level. A `CONNECT` buried in a
        // nested subquery (`… WHERE x IN (SELECT … CONNECT BY …)`) belongs to that
        // subquery, not the cursor's query, so it must not make `LEVEL`/`PRIOR`/…
        // grammatical out here. `tokens` is already the current query body, so the
        // marker counts only at its top paren depth.
        let depths = crate::ui::sql_depth::paren_depths(tokens);
        tokens.iter().enumerate().any(|(idx, token)| {
            matches!(token, SqlToken::Word(word) if word.eq_ignore_ascii_case("CONNECT"))
                && crate::ui::sql_depth::is_top_level_depth(&depths, idx)
        })
    }

    /// Best-effort type of the operand immediately before `end`, used to gate the
    /// operand-type postfix operators. Recognises string/datetime literals,
    /// datetime niladic functions, the return type of common datetime/character
    /// built-ins (by the name before a closed call), and the declared type of an
    /// in-scope column. Everything else is `Unknown`.
    fn preceding_operand_type(
        tokens: &[SqlToken],
        end: usize,
        data: &IntellisenseData,
        column_scope: &[String],
    ) -> PrecedingOperandType {
        let before = Self::meaningful_tokens_before(tokens, end);
        match before.last() {
            // A string literal is character, unless it is the body of a typed
            // datetime literal (`DATE '…'` / `TIMESTAMP '…'` / `TIME '…'`).
            Some(SqlToken::String(_)) => {
                match before.len().checked_sub(2).and_then(|i| before.get(i)) {
                    Some(SqlToken::Word(word))
                        if matches!(
                            word.to_ascii_uppercase().as_str(),
                            "DATE" | "TIMESTAMP" | "TIME"
                        ) =>
                    {
                        PrecedingOperandType::Datetime
                    }
                    _ => PrecedingOperandType::Character,
                }
            }
            Some(SqlToken::Symbol(sym)) if sym == ")" => {
                match Self::call_name_before_close(&before) {
                    Some(name) => Self::function_return_operand_type(&name),
                    None => PrecedingOperandType::Unknown,
                }
            }
            Some(SqlToken::Word(word)) => {
                let upper = word.to_ascii_uppercase();
                if upper.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                    return PrecedingOperandType::Other;
                }
                if DATETIME_VALUE_WORDS.contains(&upper.as_str()) {
                    return PrecedingOperandType::Datetime;
                }
                // An in-scope column: classify by its declared type.
                for table in column_scope {
                    if let Some(meta) = data.get_column_meta(table, &upper) {
                        return Self::classify_type_display(&meta.type_display);
                    }
                }
                PrecedingOperandType::Unknown
            }
            _ => PrecedingOperandType::Unknown,
        }
    }

    /// Map a column's `type_display` (e.g. `NUMBER(10)`, `TIMESTAMP(6) WITH TIME
    /// ZONE`, `VARCHAR2(100)`) to an operand-type family.
    fn classify_type_display(type_display: &str) -> PrecedingOperandType {
        let upper = type_display.to_ascii_uppercase();
        if upper.starts_with("DATE") || upper.contains("TIMESTAMP") {
            return PrecedingOperandType::Datetime;
        }
        if upper.contains("CHAR") || upper.contains("CLOB") {
            return PrecedingOperandType::Character;
        }
        PrecedingOperandType::Other
    }

    /// The function/identifier name immediately before the call whose closing `)`
    /// is the last token of `before`, or `None` when the `)` closes a plain
    /// parenthesised group rather than a call.
    fn call_name_before_close(before: &[&SqlToken]) -> Option<String> {
        let mut depth = 0i32;
        let mut open_index = None;
        for (index, token) in before.iter().enumerate().rev() {
            match token {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    depth -= 1;
                    if depth == 0 {
                        open_index = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let open_index = open_index?;
        match open_index.checked_sub(1).and_then(|i| before.get(i)) {
            Some(SqlToken::Word(word)) => Some(word.to_ascii_uppercase()),
            _ => None,
        }
    }

    /// Operand type produced by a named built-in. Only the unambiguous
    /// datetime/character producers are listed; anything else is `Unknown` so the
    /// type-gated operators stay withheld rather than guessed.
    fn function_return_operand_type(name: &str) -> PrecedingOperandType {
        const DATETIME_FUNCTIONS: &[&str] = &[
            "TO_DATE", "TO_TIMESTAMP", "TO_TIMESTAMP_TZ", "FROM_TZ", "ADD_MONTHS", "LAST_DAY",
            "NEXT_DAY", "NUMTODSINTERVAL", "NUMTOYMINTERVAL",
        ];
        const CHARACTER_FUNCTIONS: &[&str] = &[
            "TO_CHAR", "TO_NCHAR", "SUBSTR", "SUBSTRB", "UPPER", "LOWER", "INITCAP", "TRIM",
            "LTRIM", "RTRIM", "LPAD", "RPAD", "REPLACE", "CONCAT", "REGEXP_REPLACE",
            "REGEXP_SUBSTR", "NVL2", "TRANSLATE", "REVERSE", "SOUNDEX",
        ];
        if DATETIME_FUNCTIONS.contains(&name) {
            PrecedingOperandType::Datetime
        } else if CHARACTER_FUNCTIONS.contains(&name) {
            PrecedingOperandType::Character
        } else {
            PrecedingOperandType::Unknown
        }
    }

    /// Whether the cursor sits right after the pattern of an unclosed `LIKE`
    /// comparison (`<expr> LIKE <pattern> |`), the only place `ESCAPE` is
    /// grammatical. Scans the current predicate segment back from the cursor at
    /// paren depth 0: a `LIKE`-family keyword found before any segment boundary
    /// (a boolean connector, clause keyword or comma) means we are in an `ESCAPE`
    /// position; a boundary found first means we are not. Tokens nested inside a
    /// deeper paren level are skipped so a `LIKE` buried in a sub-expression
    /// (`f(a LIKE b) |`) never counts.
    fn cursor_follows_like_pattern(tokens: &[SqlToken], end: usize) -> bool {
        // Keywords that bound the current comparison expression: a `LIKE` before
        // one of these belongs to a different predicate, so `ESCAPE` is not valid.
        const SEGMENT_BOUNDARY: &[&str] = &[
            "AND", "OR", "WHERE", "HAVING", "ON", "WHEN", "THEN", "ELSE", "SELECT", "FROM",
            "GROUP", "ORDER", "BY", "START", "CONNECT", "SET", "VALUES", "USING", "RETURNING",
            "CASE", "INTO", "BETWEEN",
        ];
        let before = Self::meaningful_tokens_before(tokens, end);
        let mut depth = 0i32;
        for (idx, token) in before.iter().enumerate().rev() {
            match token {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                _ if depth > 0 => {}
                SqlToken::Symbol(sym) if sym == "," => return false,
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    if matches!(upper.as_str(), "LIKE" | "LIKE2" | "LIKE4" | "LIKEC") {
                        if upper == "LIKE"
                            && matches!(
                                before.get(..idx).and_then(|prefix| prefix.iter().rev().find(
                                    |token| !matches!(token, SqlToken::Comment(_))
                                )),
                                Some(SqlToken::Word(prev)) if prev.eq_ignore_ascii_case("SOUNDS")
                            )
                        {
                            return false;
                        }
                        return true;
                    }
                    if SEGMENT_BOUNDARY.contains(&upper.as_str()) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn cursor_follows_sounds_operator(tokens: &[SqlToken], end: usize) -> bool {
        matches!(
            Self::meaningful_tokens_before(tokens, end).last(),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("SOUNDS")
        )
    }

    fn mysql_trigger_pseudo_row_policy(tokens: &[SqlToken], end: usize) -> (bool, bool) {
        let mut saw_create = false;
        let mut in_trigger_header = false;
        let mut allow_new = false;
        let mut allow_old = false;

        for token in Self::meaningful_tokens_before(tokens, end) {
            match token {
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    if !in_trigger_header {
                        if upper == "CREATE" {
                            saw_create = true;
                        } else if saw_create && upper == "TRIGGER" {
                            in_trigger_header = true;
                        } else if matches!(upper.as_str(), "DROP" | "ALTER") {
                            saw_create = false;
                        }
                        continue;
                    }

                    match upper.as_str() {
                        "INSERT" => allow_new = true,
                        "UPDATE" => {
                            allow_new = true;
                            allow_old = true;
                        }
                        "DELETE" => allow_old = true,
                        "ON" => break,
                        _ => {}
                    }
                }
                SqlToken::Symbol(sym) if sym == ";" => {
                    saw_create = false;
                    in_trigger_header = false;
                    allow_new = false;
                    allow_old = false;
                }
                _ => {}
            }
        }
        (allow_new, allow_old)
    }

    fn expected_sounds_like_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Option<&'static [&'static str]> {
        const LIKE_KEYWORD: &[&str] = &["LIKE"];
        (Self::cursor_follows_sounds_operator(tokens, end)
            && crate::sql_text::mysql_compatibility_for_sql("", db_type))
        .then_some(LIKE_KEYWORD)
    }

    fn expected_sounds_like_keyword_candidates_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Option<&'static [&'static str]> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_sounds_like_keyword_candidates(tokens, end, db_type)
    }

    /// True when the cursor sits in the top-level argument list of a still-open
    /// MySQL `GROUP_CONCAT(` call and the optional `SEPARATOR` clause has not
    /// appeared yet. Nested calls inside the argument list are deliberately
    /// excluded: `GROUP_CONCAT(CONCAT(a, |))` is a `CONCAT` argument slot, not the
    /// `GROUP_CONCAT` separator slot.
    fn cursor_in_group_concat_arguments_before_separator(tokens: &[SqlToken], end: usize) -> bool {
        let limit = end.min(tokens.len());
        let mut depth = 0i32;
        let mut open_idx = None;
        for idx in (0..limit).rev() {
            match &tokens[idx] {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    if depth == 0 {
                        open_idx = Some(idx);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let Some(open_idx) = open_idx else {
            return false;
        };
        if !matches!(
            tokens.get(..open_idx)
                .unwrap_or(tokens)
                .iter()
                .rev()
                .find(|token| !matches!(token, SqlToken::Comment(_))),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("GROUP_CONCAT")
        ) {
            return false;
        }

        let mut nested = 0i32;
        for token in &tokens[(open_idx + 1)..limit] {
            match token {
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => nested += 1,
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => {
                    if nested > 0 {
                        nested -= 1;
                    }
                }
                SqlToken::Word(word)
                    if nested == 0 && word.eq_ignore_ascii_case("SEPARATOR") =>
                {
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    fn cursor_follows_quantified_comparison_operator(tokens: &[SqlToken], end: usize) -> bool {
        matches!(
            Self::meaningful_tokens_before(tokens, end).last(),
            Some(SqlToken::Symbol(sym))
                if matches!(
                    sym.as_str(),
                    "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "^="
                )
        )
    }

    /// Classify the token immediately before the cursor word: `Some(true)` when it
    /// completes an operand (a value/operator may follow), `Some(false)` when an
    /// operand/expression is expected next, `None` when it cannot be told (so both
    /// keyword families are kept to avoid hiding a valid completion).
    fn cursor_follows_complete_operand(tokens: &[SqlToken], end: usize) -> Option<bool> {
        // Operators and expression-introducing clause keywords after which a fresh
        // operand is expected (so a binary operator is *not* grammatical).
        const OPERAND_EXPECTING_PREV: &[&str] = &[
            "AND", "OR", "NOT", "IN", "IS", "LIKE", "LIKE2", "LIKE4", "LIKEC", "BETWEEN",
            "ESCAPE", "MEMBER", "SUBMULTISET", "MULTISET", "AT", "COLLATE", "DIV", "MOD", "XOR",
            "REGEXP", "RLIKE", "SOUNDS", "SEPARATOR", "AGAINST", "SELECT", "WHERE", "HAVING",
            "ON",
            "SET",
            "VALUES",
            "BY",
            "WHEN",
            "THEN",
            "ELSE",
            "START",
            "CONNECT",
            "RETURNING",
            "USING",
            "AS",
            "CALL",
            "RETURN",
            "DATE",
            "TIME",
            "TIMESTAMP",
            "INTERVAL",
            "BINARY",
        ];
        // Keywords that themselves complete an operand (a literal/pseudo-column).
        const VALUE_COMPLETE_PREV: &[&str] = &[
            "NULL", "TRUE", "FALSE", "UNKNOWN", "LEVEL", "ROWNUM", "ROWID", "DUAL", "DEFAULT",
        ];
        match Self::meaningful_tokens_before(tokens, end).last() {
            None => Some(false),
            Some(SqlToken::String(_)) => Some(true),
            Some(SqlToken::Comment(_)) => None,
            Some(SqlToken::Symbol(sym)) => match sym.as_str() {
                ")" | "]" => Some(true),
                "(" | "," => Some(false),
                "." => None,
                // Arithmetic/comparison/concatenation operators expect an operand.
                _ => Some(false),
            },
            Some(SqlToken::Word(word)) => {
                let upper = word.to_ascii_uppercase();
                if upper.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                    return Some(true);
                }
                if VALUE_COMPLETE_PREV.contains(&upper.as_str()) {
                    return Some(true);
                }
                if DATETIME_VALUE_WORDS.contains(&upper.as_str()) {
                    return Some(true);
                }
                if PARENTHESIZED_EXPRESSION_CONSTRUCT_WORDS.contains(&upper.as_str()) {
                    return Some(false);
                }
                if !Self::token_is_language_keyword(&upper) {
                    return Some(true);
                }
                if OPERAND_EXPECTING_PREV.contains(&upper.as_str()) {
                    return Some(false);
                }
                None
            }
        }
    }

    /// Dialect-agnostic check used only to tell an identifier from a reserved
    /// word while classifying the previous token. The union of both catalogs is
    /// deliberate: a word that is a keyword in either dialect is treated as a
    /// keyword, which at worst leaves the position ambiguous (both keyword
    /// families kept) and so can never hide a valid completion.
    fn token_is_language_keyword(upper: &str) -> bool {
        crate::sql_text::ORACLE_SQL_KEYWORDS
            .binary_search(&upper)
            .is_ok()
            || crate::sql_text::MYSQL_SQL_KEYWORDS
                .binary_search(&upper)
                .is_ok()
    }

    /// Whether the word `upper` immediately before a `(` introduces a *function
    /// call*, as opposed to a grouping/predicate paren or a subquery (`SELECT
    /// (…)`, `IN (…)`, `EXISTS (…)`, `VALUES (…)`). A catalog function
    /// (`COUNT`/`SUM`/`UPPER`), a user-defined routine (any non-keyword
    /// identifier), and the full-text operator `MATCH` (a keyword that is not in
    /// the function catalog) all introduce a call; clause/operator keywords do
    /// not. Distinguishing the two is what keeps the call-continuation keywords
    /// (`OVER`/`KEEP`/`WITHIN`) and the set-quantifiers (`DISTINCT`/`UNIQUE`) off
    /// a plain `(a + b)` / `(SELECT …)` paren.
    fn name_introduces_call(
        data: &IntellisenseData,
        upper: &str,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        data.is_language_function(upper, db_type)
            || !Self::token_is_language_keyword(upper)
            || upper == "MATCH"
    }

    fn qualifier_matches_visible_relation_scope(
        qualifier: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> bool {
        deep_ctx.tables_in_scope.iter().any(|table_ref| {
            Self::completion_identifiers_match(&table_ref.name, qualifier)
                || table_ref
                    .alias
                    .as_deref()
                    .is_some_and(|alias| Self::completion_identifiers_match(alias, qualifier))
        }) || deep_ctx
            .ctes
            .iter()
            .any(|cte| Self::completion_identifiers_match(&cte.name, qualifier))
            || deep_ctx
                .subqueries
                .iter()
                .any(|subq| Self::completion_identifiers_match(&subq.alias, qualifier))
    }

    fn resolve_qualified_completion_mode(
        qualifier: &str,
        context: SqlContext,
        deep_ctx: &intellisense_context::CursorContext,
        data: &IntellisenseData,
    ) -> Option<QualifiedCompletionMode> {
        if let Some(kind) = Self::expected_object_suggestion_kind("", Some(qualifier), deep_ctx) {
            if data.has_members_for_qualifier(qualifier, false) {
                return Some(QualifiedCompletionMode::ObjectMembers);
            }
            if Self::expected_qualifier_member_kinds(kind).is_some()
                && data.has_members_for_qualifier(qualifier, true)
            {
                return Some(QualifiedCompletionMode::RelationMembers);
            }
        }

        if matches!(context, SqlContext::TableName)
            && data.has_members_for_qualifier(qualifier, true)
        {
            return Some(QualifiedCompletionMode::RelationMembers);
        }

        if Self::qualifier_matches_visible_relation_scope(qualifier, deep_ctx)
            || data.is_known_relation(qualifier)
        {
            return Some(QualifiedCompletionMode::RelationColumns);
        }

        let resolved_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        if resolved_tables
            .iter()
            .any(|table| data.is_known_relation(table))
        {
            return Some(QualifiedCompletionMode::RelationColumns);
        }

        if data.has_members_for_qualifier(qualifier, false) {
            return Some(QualifiedCompletionMode::ObjectMembers);
        }

        None
    }

    fn previous_meaningful_words_upper(
        tokens: &[SqlToken],
        end: usize,
        max_words: usize,
    ) -> Vec<String> {
        if max_words == 0 {
            return Vec::new();
        }

        let mut words_rev = Vec::new();
        for token in tokens.get(..end).unwrap_or(tokens).iter().rev() {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => {
                    words_rev.push(word.to_ascii_uppercase());
                    if words_rev.len() >= max_words {
                        break;
                    }
                }
                SqlToken::Symbol(_) => {}
                _ => break,
            }
        }
        words_rev.reverse();
        words_rev
    }

    fn expected_row_limiting_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const FETCH_KEYWORDS: &[&str] = &["FETCH"];
        const OFFSET_KEYWORDS: &[&str] = &["OFFSET"];
        const ROW_UNIT_KEYWORDS: &[&str] = &["ROW", "ROWS"];
        const ONLY_WITH_KEYWORDS: &[&str] = &["ONLY", "WITH"];
        const TIES_KEYWORDS: &[&str] = &["TIES"];

        let words = Self::previous_meaningful_words_with_bind_markers_upper(tokens, end, 5);
        let len = words.len();
        let tail = |from_end: usize| len.checked_sub(from_end).and_then(|idx| words.get(idx));
        let word = |from_end: usize| {
            tail(from_end).map(|(value, _)| value.as_str())
        };
        let is_count = |from_end: usize| {
            tail(from_end)
                .is_some_and(|(value, is_bind)| Self::is_row_count_tail_word(value, *is_bind))
        };

        if len >= 2
            && word(2).is_some_and(Self::is_row_limit_unit)
            && word(1) == Some("WITH")
        {
            return Some(TIES_KEYWORDS);
        }

        if len >= 5
            && word(5) == Some("FETCH")
            && word(4).is_some_and(Self::is_fetch_row_limit_direction)
            && is_count(3)
            && word(2) == Some("PERCENT")
            && word(1).is_some_and(Self::is_row_limit_unit)
        {
            return Some(ONLY_WITH_KEYWORDS);
        }

        if len >= 4
            && word(4) == Some("FETCH")
            && word(3).is_some_and(Self::is_fetch_row_limit_direction)
            && is_count(2)
            && word(1).is_some_and(Self::is_row_limit_unit)
        {
            return Some(ONLY_WITH_KEYWORDS);
        }

        if len >= 3
            && word(3) == Some("FETCH")
            && word(2).is_some_and(Self::is_fetch_row_limit_direction)
            && word(1).is_some_and(Self::is_row_limit_unit)
        {
            return Some(ONLY_WITH_KEYWORDS);
        }

        if len >= 4
            && word(4) == Some("FETCH")
            && word(3).is_some_and(Self::is_fetch_row_limit_direction)
            && is_count(2)
            && word(1) == Some("PERCENT")
        {
            return Some(ROW_UNIT_KEYWORDS);
        }

        if len >= 3
            && word(3) == Some("FETCH")
            && word(2).is_some_and(Self::is_fetch_row_limit_direction)
            && is_count(1)
        {
            return Some(ROW_UNIT_KEYWORDS);
        }

        if len >= 2
            && word(2) == Some("FETCH")
            && word(1).is_some_and(Self::is_fetch_row_limit_direction)
        {
            return Some(ROW_UNIT_KEYWORDS);
        }

        if len >= 3
            && word(3) == Some("OFFSET")
            && is_count(2)
            && word(1).is_some_and(Self::is_row_limit_unit)
        {
            return Some(FETCH_KEYWORDS);
        }

        if len >= 2
            && word(2) == Some("OFFSET")
            && is_count(1)
            && !(len >= 4 && word(4) == Some("LIMIT"))
        {
            return Some(ROW_UNIT_KEYWORDS);
        }

        if len >= 2 && word(2) == Some("LIMIT") && is_count(1) {
            return Some(OFFSET_KEYWORDS);
        }

        None
    }

    /// True when the cursor sits directly inside the parentheses of a window
    /// specification — either an inline `OVER (...)` or a named definition in a
    /// `WINDOW name AS (...)` clause. Used to gate window-frame keyword hints
    /// precisely, instead of merely checking that some `OVER` appears earlier in
    /// the statement (which misfires after a closed `OVER ()` on a column named
    /// `rows`/`current`/...). Paren depth is tracked so a `WINDOW` clause inside a
    /// subquery is recognized while a `WITH cte AS (...)` body is not.
    fn window_spec_open_paren_index(tokens: &[SqlToken], end: usize) -> Option<usize> {
        // Stack entry = whether this open paren is a window-specification paren,
        // plus the token index of the open paren.
        let mut spec_paren_stack: Vec<(bool, usize)> = Vec::new();
        let mut last_word_was_over = false;
        let mut last_word_was_as = false;
        // Paren depth at which a `WINDOW` clause is currently open, so only the
        // `name AS (` parens belonging to that clause count as window specs.
        let mut window_clause_depth: Option<usize> = None;
        for (idx, token) in tokens.get(..end).unwrap_or(tokens).iter().enumerate() {
            let depth = spec_paren_stack.len();
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    let is_window_spec = last_word_was_over
                        || (last_word_was_as && window_clause_depth == Some(depth));
                    spec_paren_stack.push((is_window_spec, idx));
                    last_word_was_over = false;
                    last_word_was_as = false;
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    spec_paren_stack.pop();
                    last_word_was_over = false;
                    last_word_was_as = false;
                    if window_clause_depth.is_some_and(|d| spec_paren_stack.len() < d) {
                        window_clause_depth = None;
                    }
                }
                SqlToken::Symbol(sym) if sym == ";" && depth == 0 => {
                    window_clause_depth = None;
                    last_word_was_over = false;
                    last_word_was_as = false;
                }
                SqlToken::Word(word) => {
                    last_word_was_over = word.eq_ignore_ascii_case("OVER");
                    last_word_was_as = word.eq_ignore_ascii_case("AS");
                    if word.eq_ignore_ascii_case("WINDOW") {
                        window_clause_depth = Some(depth);
                    }
                }
                _ => {
                    last_word_was_over = false;
                    last_word_was_as = false;
                }
            }
        }
        spec_paren_stack
            .last()
            .and_then(|(is_window_spec, idx)| is_window_spec.then_some(*idx))
    }

    fn cursor_is_inside_window_spec(tokens: &[SqlToken], end: usize) -> bool {
        Self::window_spec_open_paren_index(tokens, end).is_some()
    }

    /// True when the cursor sits at the very start of a window specification —
    /// immediately after the `OVER (` / `WINDOW name AS (` open paren, before any
    /// `PARTITION`/`ORDER`/frame keyword has been typed. Only the clause openers
    /// (`PARTITION BY`, `ORDER BY`, `ROWS`/`RANGE`/`GROUPS`) or a window-name
    /// reference are grammatical there; a bare column is never valid, so the
    /// column list the surrounding expression phase would offer is suppressed and
    /// the openers are emitted instead. Gated through `cursor_is_inside_window_spec`
    /// so a parenthesised expression outside a window never triggers it, and on the
    /// previous token being the open paren so it stops applying once the clause
    /// body (`PARTITION BY |`, `ORDER BY |`, …) begins.
    fn cursor_is_at_window_spec_start(tokens: &[SqlToken], end: usize) -> bool {
        if !Self::cursor_is_inside_window_spec(tokens, end) {
            return false;
        }
        matches!(
            Self::meaningful_tokens_before(tokens, end).last(),
            Some(SqlToken::Symbol(sym)) if sym == "("
        )
    }

    fn cursor_is_at_window_spec_start_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::cursor_is_at_window_spec_start(tokens, end)
    }

    /// Clause-opener hints for the start of a window specification (`OVER (|)` /
    /// `WINDOW name AS (|)`). Mirrors the suppression arm
    /// `cursor_is_at_window_spec_start` so identifier suppression cannot drift
    /// from keyword emission.
    fn expected_window_spec_start_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const OPENERS: &[&str] = &["PARTITION BY", "ORDER BY", "ROWS", "RANGE", "GROUPS"];
        if Self::cursor_is_at_window_spec_start(tokens, end) {
            Some(OPENERS)
        } else {
            None
        }
    }

    fn window_partition_tail_can_transition_to_order_by(
        tokens: &[SqlToken],
        end: usize,
    ) -> bool {
        let Some(parts) = Self::window_spec_top_level_parts(tokens, end) else {
            return false;
        };
        let Some(partition_idx) = parts.windows(2).position(|pair| {
            pair[0] == "PARTITION" && pair[1] == "BY"
        }) else {
            return false;
        };
        let tail = &parts[partition_idx + 2..];
        if tail.is_empty()
            || tail.iter().any(|part| {
                part == "ORDER" || part == "PARTITION" || Self::is_window_frame_unit(part)
            })
            || matches!(
                tail.last().map(String::as_str),
                Some("," | "." | "BY" | "AND" | "OR" | "NOT" | "BETWEEN" | "IN" | "LIKE")
            )
        {
            return false;
        }
        Self::cursor_follows_complete_operand(tokens, end) != Some(false)
    }

    /// Clause-transition hint after a completed `PARTITION BY` expression inside
    /// a window spec. The spec start (`OVER (|)`) and fixed `PARTITION |` /
    /// `ORDER |` continuations are handled by their own scoped slots; this covers
    /// only the later optional `ORDER BY` clause and keeps earlier-clause words
    /// out of frame/order bodies.
    fn expected_window_spec_clause_transition_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const AFTER_PARTITION_EXPR: &[&str] = &["ORDER BY"];
        Self::window_partition_tail_can_transition_to_order_by(tokens, end)
            .then_some(AFTER_PARTITION_EXPR)
    }

    fn expected_window_spec_clause_transition_candidates_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<&'static [&'static str]> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_window_spec_clause_transition_candidates(tokens, end)
    }

    /// The PL/SQL `%`-attribute hints at the cursor, if any — `<var>%|` /
    /// `<table>%|` -> `TYPE`/`ROWTYPE`, `<table>.<col>%|` -> `TYPE` (a column has
    /// no row type). Anchored on the `%` being preceded by an identifier chain
    /// that itself sits at a data-type slot, so the modulo operator in an
    /// expression (`v := a % |`, never a type slot) is left as a value position.
    /// Only `TYPE`/`ROWTYPE` are grammatical after the attribute `%`, so the
    /// matching suppression arm clears every other identifier source there.
    fn type_attribute_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const DOTTED: &[&str] = &["TYPE"];
        const SIMPLE: &[&str] = &["TYPE", "ROWTYPE"];

        let indexed: Vec<(usize, &SqlToken)> = tokens
            .get(..end.min(tokens.len()))
            .unwrap_or(tokens)
            .iter()
            .enumerate()
            .filter(|(_, token)| !matches!(token, SqlToken::Comment(_)))
            .collect();
        let is_word = |token: &SqlToken| matches!(token, SqlToken::Word(_));
        let is_symbol = |token: &SqlToken, sym: &str| {
            matches!(token, SqlToken::Symbol(value) if value == sym)
        };

        let mut i = indexed.len().checked_sub(1)?;
        if !is_symbol(indexed[i].1, "%") {
            return None;
        }
        // The identifier immediately before `%`.
        i = i.checked_sub(1)?;
        if !is_word(indexed[i].1) {
            return None;
        }
        let mut has_dot = false;
        let mut chain_start = indexed[i].0;
        // Walk back over any `. <identifier>` qualifier links.
        while i >= 2 && is_symbol(indexed[i - 1].1, ".") && is_word(indexed[i - 2].1) {
            has_dot = true;
            i -= 2;
            chain_start = indexed[i].0;
        }
        // The chain only forms a `%TYPE`/`%ROWTYPE` attribute where the chain
        // itself began at a data-type slot; reuse the type-position detector at the
        // slot start so a modulo operand is never mistaken for an attribute.
        if Self::data_type_position(tokens, chain_start).is_some() {
            Some(if has_dot { DOTTED } else { SIMPLE })
        } else {
            None
        }
    }

    fn cursor_is_at_type_attribute_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::type_attribute_candidates(tokens, end).is_some()
    }

    /// Privilege hints for the `GRANT |` / `REVOKE |` privilege list, before the
    /// `ON`/`TO`/`FROM` separator. Emission only — a role name is also valid here
    /// (`GRANT my_role TO u`), so identifiers are NOT suppressed; the privileges
    /// are merged alongside them. Anchored on the controlling verb with no `ON`/
    /// `TO`/`FROM` seen yet and the cursor right after the verb or a list comma.
    fn expected_grant_privilege_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const PRIVILEGES: &[&str] = &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "REFERENCES",
            "ALTER",
            "INDEX",
            "EXECUTE",
            "READ",
            "ALL",
            "ALL PRIVILEGES",
        ];
        let toks = Self::meaningful_tokens_before(tokens, end);
        // The cursor must sit right after the verb (`GRANT |`) or a privilege-list
        // comma (`GRANT SELECT, |`).
        let after_verb_or_comma = matches!(
            toks.last(),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("GRANT") || word.eq_ignore_ascii_case("REVOKE")
        ) || matches!(toks.last(), Some(SqlToken::Symbol(sym)) if sym == ",");
        if !after_verb_or_comma {
            return None;
        }
        // Scan back to the controlling verb; bail if a clause separator already
        // moved the cursor past the privilege list.
        for token in toks.iter().rev() {
            if let SqlToken::Word(word) = token {
                let upper = word.to_ascii_uppercase();
                match upper.as_str() {
                    "GRANT" | "REVOKE" => return Some(PRIVILEGES),
                    "ON" | "TO" | "FROM" => return None,
                    _ => {}
                }
            }
        }
        None
    }

    /// True when the cursor is at the brand-new object name of a `CREATE`
    /// statement — the slot right after the leaf object-type keyword (`CREATE
    /// TABLE |`, `CREATE OR REPLACE PACKAGE |`, `CREATE MATERIALIZED VIEW |`,
    /// `CREATE TABLE IF NOT EXISTS |`, …). The name is brand new, so existing
    /// relations/objects are never valid there and are suppressed. Scoped to a
    /// `CREATE` statement (so `DROP`/`ALTER <type> <existing>` keep their existing-
    /// object completion) and to the leaf type keywords that directly introduce a
    /// name (a non-leaf such as `MATERIALIZED`/`GLOBAL`/`OR REPLACE` still expects
    /// a following keyword, handled by the keyword merge).
    fn cursor_is_at_create_object_new_name(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        const NAME_INTRODUCERS: &[&str] = &[
            "TABLE",
            "VIEW",
            "INDEX",
            "SEQUENCE",
            "SYNONYM",
            "PROCEDURE",
            "FUNCTION",
            "TRIGGER",
            "PACKAGE",
            "TYPE",
            "BODY",
            "DATABASE",
            "TABLESPACE",
            "USER",
            "ROLE",
            "PROFILE",
            "DIRECTORY",
            "CONTEXT",
            "CLUSTER",
            "DIMENSION",
            "OPERATOR",
            "LIBRARY",
            // MySQL/`CREATE … IF NOT EXISTS <name>`.
            "EXISTS",
        ];
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        let words = Self::previous_meaningful_words_upper(tokens, end, 12);
        // Statement must be a CREATE (not DROP/ALTER, whose `<type> <name>` slots
        // reference an existing object), and the cursor must be past the type
        // keyword (`CREATE |` itself still wants an object-type keyword).
        if words.first().map(String::as_str) != Some("CREATE") || words.len() < 2 {
            return false;
        }
        words
            .last()
            .is_some_and(|word| NAME_INTRODUCERS.contains(&word.as_str()))
    }

    fn is_window_frame_unit(word: &str) -> bool {
        matches!(word, "ROWS" | "RANGE" | "GROUPS")
    }

    fn window_spec_top_level_parts(tokens: &[SqlToken], end: usize) -> Option<Vec<String>> {
        let open_idx = Self::window_spec_open_paren_index(tokens, end)?;
        let depths = crate::ui::sql_depth::paren_depths(tokens);
        let top_depth = crate::ui::sql_depth::depth_at(&depths, open_idx) + 1;
        let mut parts = Vec::new();
        for (idx, token) in tokens.iter().enumerate().take(end).skip(open_idx + 1) {
            if crate::ui::sql_depth::depth_at(&depths, idx) != top_depth {
                continue;
            }
            match token {
                SqlToken::Word(word) => parts.push(word.to_ascii_uppercase()),
                SqlToken::Symbol(sym) => parts.push(sym.clone()),
                SqlToken::Comment(_) | SqlToken::String(_) => {}
            }
        }
        Some(parts)
    }

    fn window_order_by_sort_modifier_slot(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<WindowOrderBySortModifierSlot> {
        let parts = Self::window_spec_top_level_parts(tokens, end)?;
        let order_idx = parts
            .windows(2)
            .enumerate()
            .rev()
            .find_map(|(idx, pair)| (pair[0] == "ORDER" && pair[1] == "BY").then_some(idx))?;
        let order_tail = &parts[order_idx + 2..];
        if order_tail.is_empty() || order_tail.iter().any(|part| Self::is_window_frame_unit(part)) {
            return None;
        }
        let sort_tail = match order_tail.iter().rposition(|part| part == ",") {
            Some(comma_idx) => &order_tail[comma_idx + 1..],
            None => order_tail,
        };
        if sort_tail.is_empty() {
            return None;
        }

        match sort_tail {
            [prefix @ .., last]
                if last == "NULLS" && !prefix.is_empty() =>
            {
                Some(WindowOrderBySortModifierSlot::AfterNulls)
            }
            [prefix @ .., prev, last]
                if prev == "NULLS"
                    && matches!(last.as_str(), "FIRST" | "LAST")
                    && !prefix.is_empty() =>
            {
                Some(WindowOrderBySortModifierSlot::AfterNullOrdering)
            }
            [.., last] if matches!(last.as_str(), "ASC" | "DESC") => {
                Some(WindowOrderBySortModifierSlot::AfterDirection)
            }
            [.., last]
                if !matches!(
                    last.as_str(),
                    "ORDER" | "BY" | "NULLS" | "ASC" | "DESC" | "FIRST" | "LAST" | ","
                ) =>
            {
                Some(WindowOrderBySortModifierSlot::AfterSortKey)
            }
            _ => None,
        }
    }

    fn window_order_by_sort_modifier_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<WindowOrderBySortModifierSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::window_order_by_sort_modifier_slot(tokens, end)
    }

    fn window_frame_tail_has_complete_bound(tail: &[String]) -> bool {
        match tail {
            [.., prev, last] if prev == "CURRENT" && last == "ROW" => true,
            [.., last] if matches!(last.as_str(), "PRECEDING" | "FOLLOWING") => true,
            _ => false,
        }
    }

    fn window_frame_bound_expression_awaits_direction(segment: &[String]) -> bool {
        let Some(last) = segment.last() else {
            return false;
        };
        !matches!(
            last.as_str(),
            "BETWEEN"
                | "AND"
                | "UNBOUNDED"
                | "CURRENT"
                | "ROW"
                | "PRECEDING"
                | "FOLLOWING"
                | "EXCLUDE"
                | "NO"
                | "OTHERS"
                | "GROUP"
                | "TIES"
                | ","
                | "("
                | "+"
                | "-"
                | "*"
                | "/"
                | "%"
        )
    }

    fn window_frame_slot_from_tail(tail: &[String]) -> Option<WindowFrameKeywordSlot> {
        if tail.is_empty() {
            return Some(WindowFrameKeywordSlot::AfterUnit);
        }
        if let Some(exclude_idx) = tail.iter().rposition(|part| part == "EXCLUDE") {
            return match &tail[exclude_idx + 1..] {
                [] => Some(WindowFrameKeywordSlot::AfterExclude),
                [word] if word == "CURRENT" => Some(WindowFrameKeywordSlot::AfterExcludeCurrent),
                [word] if word == "NO" => Some(WindowFrameKeywordSlot::AfterExcludeNo),
                [word] if matches!(word.as_str(), "GROUP" | "TIES") => {
                    Some(WindowFrameKeywordSlot::AfterExcludeEnd)
                }
                [first, second] if first == "CURRENT" && second == "ROW" => {
                    Some(WindowFrameKeywordSlot::AfterExcludeEnd)
                }
                [first, second] if first == "NO" && second == "OTHERS" => {
                    Some(WindowFrameKeywordSlot::AfterExcludeEnd)
                }
                _ => None,
            };
        }
        match tail.last().map(String::as_str) {
            Some("UNBOUNDED") => return Some(WindowFrameKeywordSlot::AfterUnbounded),
            Some("CURRENT") => return Some(WindowFrameKeywordSlot::AfterCurrent),
            Some("BETWEEN") => return Some(WindowFrameKeywordSlot::AfterBetween),
            Some("AND") if tail.first().is_some_and(|part| part == "BETWEEN") => {
                return Some(WindowFrameKeywordSlot::AfterAnd);
            }
            _ => {}
        }
        if Self::window_frame_tail_has_complete_bound(tail) {
            if tail.first().is_some_and(|part| part == "BETWEEN")
                && !tail.iter().any(|part| part == "AND")
            {
                Some(WindowFrameKeywordSlot::AfterFirstBound)
            } else {
                Some(WindowFrameKeywordSlot::AfterFrameEnd)
            }
        } else if let Some(and_idx) = tail.iter().rposition(|part| part == "AND") {
            Self::window_frame_bound_expression_awaits_direction(&tail[and_idx + 1..])
                .then_some(WindowFrameKeywordSlot::AfterBoundExpression)
        } else if tail.first().is_some_and(|part| part == "BETWEEN") {
            Self::window_frame_bound_expression_awaits_direction(&tail[1..])
                .then_some(WindowFrameKeywordSlot::AfterBoundExpression)
        } else if Self::window_frame_bound_expression_awaits_direction(tail) {
            Some(WindowFrameKeywordSlot::AfterBoundExpression)
        } else {
            None
        }
    }

    fn window_frame_keyword_slot(tokens: &[SqlToken], end: usize) -> Option<WindowFrameKeywordSlot> {
        let parts = Self::window_spec_top_level_parts(tokens, end)?;
        let unit_idx = parts
            .iter()
            .rposition(|part| Self::is_window_frame_unit(part))?;
        Self::window_frame_slot_from_tail(&parts[unit_idx + 1..])
    }

    /// Window-frame keyword hints inside an `OVER (... ROWS|RANGE|GROUPS ...)`
    /// clause. The classifier is anchored to the active frame unit inside the
    /// current window-spec paren, so lookalike sort expressions such as
    /// `ORDER BY current |` do not receive frame keywords.
    fn expected_window_frame_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        Self::window_frame_keyword_slot(tokens, end).map(window_frame_keywords_for)
    }

    /// True when the cursor is at a window-frame slot that accepts only a fixed
    /// frame keyword, never a column or value. Value-bound slots (`ROWS |`,
    /// `BETWEEN |`, `... AND |`) keep columns visible; completed frame and
    /// `EXCLUDE` tails suppress them until a delimiter or fixed keyword is typed.
    fn cursor_is_at_window_frame_keyword_only_position(tokens: &[SqlToken], end: usize) -> bool {
        Self::window_frame_keyword_slot(tokens, end).is_some_and(window_frame_slot_suppresses_columns)
    }

    fn cursor_is_at_window_frame_keyword_only_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::cursor_is_at_window_frame_keyword_only_position(tokens, end)
    }

    fn keep_dense_rank_top_level_words(tokens: &[SqlToken], end: usize) -> Option<Vec<String>> {
        let mut paren_stack: Vec<(usize, Option<String>)> = Vec::new();
        let mut last_word: Option<String> = None;
        for (idx, token) in tokens.get(..end).unwrap_or(tokens).iter().enumerate() {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    paren_stack.push((idx, last_word.take()));
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    paren_stack.pop();
                    last_word = None;
                }
                SqlToken::Word(word) => last_word = Some(word.clone()),
                _ => last_word = None,
            }
        }
        let (open_idx, Some(preceding_word)) = paren_stack.last()? else {
            return None;
        };
        if !preceding_word.eq_ignore_ascii_case("KEEP") {
            return None;
        }

        let depths = crate::ui::sql_depth::paren_depths(tokens);
        let top_depth = crate::ui::sql_depth::depth_at(&depths, *open_idx) + 1;
        let mut words = Vec::new();
        for (idx, token) in tokens.iter().enumerate().take(end).skip(open_idx + 1) {
            if crate::ui::sql_depth::depth_at(&depths, idx) != top_depth {
                continue;
            }
            if let SqlToken::Word(word) = token {
                words.push(word.to_ascii_uppercase());
            }
        }
        Some(words)
    }

    fn keep_dense_rank_slot(tokens: &[SqlToken], end: usize) -> Option<KeepDenseRankSlot> {
        match Self::keep_dense_rank_top_level_words(tokens, end)?.as_slice() {
            [word] if word == "DENSE_RANK" => Some(KeepDenseRankSlot::AfterDenseRank),
            [dense_rank, direction]
                if dense_rank == "DENSE_RANK"
                    && matches!(direction.as_str(), "FIRST" | "LAST") =>
            {
                Some(KeepDenseRankSlot::AfterRankDirection)
            }
            _ => None,
        }
    }

    fn keep_dense_rank_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<KeepDenseRankSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::keep_dense_rank_slot(tokens, end)
    }

    fn function_supports_within_group(name: &str) -> bool {
        matches!(
            name,
            "LISTAGG"
                | "PERCENTILE_CONT"
                | "PERCENTILE_DISC"
                | "RANK"
                | "DENSE_RANK"
                | "CUME_DIST"
                | "PERCENT_RANK"
        )
    }

    fn ordered_set_call_before(tokens: &[&SqlToken]) -> bool {
        matches!(tokens.last(), Some(SqlToken::Symbol(sym)) if sym == ")")
            && Self::call_name_before_close(tokens)
                .is_some_and(|name| Self::function_supports_within_group(&name))
    }

    fn within_group_slot(tokens: &[SqlToken], end: usize) -> Option<WithinGroupSlot> {
        let toks = Self::meaningful_tokens_before(tokens, end);
        match toks.as_slice() {
            [prefix @ .., SqlToken::Word(word)] if word.eq_ignore_ascii_case("WITHIN") => {
                Self::ordered_set_call_before(prefix).then_some(WithinGroupSlot::AfterWithin)
            }
            [prefix @ .., SqlToken::Word(within), SqlToken::Word(group)]
                if within.eq_ignore_ascii_case("WITHIN") && group.eq_ignore_ascii_case("GROUP") =>
            {
                Self::ordered_set_call_before(prefix).then_some(WithinGroupSlot::AfterGroup)
            }
            _ => None,
        }
    }

    fn within_group_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<WithinGroupSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::within_group_slot(tokens, end)
    }

    fn analytic_null_treatment_call_kind(name: &str) -> Option<AnalyticNullTreatmentCall> {
        match name {
            "NTH_VALUE" => Some(AnalyticNullTreatmentCall::NthValue),
            "FIRST_VALUE" | "LAST_VALUE" | "LAG" | "LEAD" => {
                Some(AnalyticNullTreatmentCall::General)
            }
            _ => None,
        }
    }

    fn closed_analytic_null_treatment_call(
        tokens: &[&SqlToken],
    ) -> Option<AnalyticNullTreatmentCall> {
        matches!(tokens.last(), Some(SqlToken::Symbol(sym)) if sym == ")")
            .then(|| Self::call_name_before_close(tokens))
            .flatten()
            .and_then(|name| Self::analytic_null_treatment_call_kind(&name))
    }

    fn nth_value_from_direction_tail(tokens: &[&SqlToken]) -> bool {
        match tokens {
            [prefix @ .., SqlToken::Word(from), SqlToken::Word(direction)]
                if from.eq_ignore_ascii_case("FROM")
                    && matches!(direction.to_ascii_uppercase().as_str(), "FIRST" | "LAST") =>
            {
                Self::closed_analytic_null_treatment_call(prefix)
                    == Some(AnalyticNullTreatmentCall::NthValue)
            }
            _ => false,
        }
    }

    fn analytic_null_treatment_can_follow(tokens: &[&SqlToken]) -> bool {
        Self::closed_analytic_null_treatment_call(tokens).is_some()
            || Self::nth_value_from_direction_tail(tokens)
    }

    fn analytic_null_treatment_written(tokens: &[&SqlToken]) -> bool {
        match tokens {
            [prefix @ .., SqlToken::Word(treatment)]
                if matches!(
                    treatment.to_ascii_uppercase().as_str(),
                    "IGNORE" | "RESPECT"
                ) =>
            {
                Self::analytic_null_treatment_can_follow(prefix)
            }
            _ => false,
        }
    }

    fn analytic_null_treatment_slot(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<AnalyticNullTreatmentSlot> {
        let toks = Self::meaningful_tokens_before(tokens, end);
        match toks.as_slice() {
            [prefix @ .., SqlToken::Word(nulls)] if nulls.eq_ignore_ascii_case("NULLS") => {
                Self::analytic_null_treatment_written(prefix)
                    .then_some(AnalyticNullTreatmentSlot::AfterNulls)
            }
            [prefix @ .., SqlToken::Word(treatment)]
                if matches!(
                    treatment.to_ascii_uppercase().as_str(),
                    "IGNORE" | "RESPECT"
                ) =>
            {
                Self::analytic_null_treatment_can_follow(prefix)
                    .then_some(AnalyticNullTreatmentSlot::AfterNullTreatment)
            }
            [prefix @ .., SqlToken::Word(from)] if from.eq_ignore_ascii_case("FROM") => {
                (Self::closed_analytic_null_treatment_call(prefix)
                    == Some(AnalyticNullTreatmentCall::NthValue))
                .then_some(AnalyticNullTreatmentSlot::AfterNthValueFrom)
            }
            _ if Self::nth_value_from_direction_tail(&toks) => {
                Some(AnalyticNullTreatmentSlot::AfterNthValueFromDirection)
            }
            _ => Self::closed_analytic_null_treatment_call(&toks).map(|call| match call {
                AnalyticNullTreatmentCall::General => {
                    AnalyticNullTreatmentSlot::AfterAnalyticCall
                }
                AnalyticNullTreatmentCall::NthValue => {
                    AnalyticNullTreatmentSlot::AfterNthValueCall
                }
            }),
        }
    }

    fn analytic_null_treatment_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<AnalyticNullTreatmentSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::analytic_null_treatment_slot(tokens, end)
    }

    /// The word immediately preceding the innermost still-open paren at the
    /// cursor (e.g. `ADD` for `ADD (col |)`), or `None` at top level.
    fn innermost_open_paren_preceding_word(tokens: &[SqlToken], end: usize) -> Option<String> {
        let mut preceding_word_stack: Vec<Option<String>> = Vec::new();
        let mut last_word: Option<String> = None;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    preceding_word_stack.push(last_word.take());
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    preceding_word_stack.pop();
                    last_word = None;
                }
                SqlToken::Word(word) => last_word = Some(word.clone()),
                _ => last_word = None,
            }
        }
        preceding_word_stack.last().cloned().flatten()
    }

    /// Non-comment tokens strictly before `end`, in order.
    fn meaningful_tokens_before(tokens: &[SqlToken], end: usize) -> Vec<&SqlToken> {
        tokens
            .get(..end)
            .unwrap_or(tokens)
            .iter()
            .filter(|token| !matches!(token, SqlToken::Comment(_)))
            .collect()
    }

    /// Words that occupy a column-name slot in DDL but are not column names, so a
    /// following cursor is not a data-type position (`CONSTRAINT pk ...`, etc.).
    fn is_ddl_structural_keyword(word: &str) -> bool {
        matches!(
            word.to_ascii_uppercase().as_str(),
            "CONSTRAINT"
                | "PRIMARY"
                | "FOREIGN"
                | "UNIQUE"
                | "CHECK"
                | "KEY"
                | "INDEX"
                | "COLUMN"
                | "ADD"
                | "MODIFY"
                | "CHANGE"
                | "DROP"
                | "RENAME"
                | "NOT"
                | "NULL"
                | "DEFAULT"
                | "REFERENCES"
                | "TABLE"
                | "AS"
                | "SELECT"
                | "PERIOD"
                | "PARTITION"
                | "USING"
                | "ENABLE"
                | "DISABLE"
        )
    }

    /// Classify whether the cursor sits where a SQL data type is expected, and in
    /// which kind of position (the keyword set differs for some dialects).
    fn data_type_position(tokens: &[SqlToken], end: usize) -> Option<DataTypePosition> {
        if Self::cursor_is_after_cast_as(tokens, end) {
            return Some(DataTypePosition::Cast);
        }
        if Self::cursor_is_at_ddl_column_type(tokens, end) {
            return Some(DataTypePosition::ColumnDef);
        }
        if Self::cursor_is_in_table_function_columns_type(tokens, end) {
            return Some(DataTypePosition::ColumnDef);
        }
        if Self::cursor_is_at_plsql_type(tokens, end) {
            return Some(DataTypePosition::Plsql);
        }
        if Self::cursor_is_at_json_returning_type(tokens, end) {
            return Some(DataTypePosition::Cast);
        }
        None
    }

    /// True when the cursor is at the type slot of a JSON function's `RETURNING`
    /// clause — `JSON_VALUE(col, '$.a' RETURNING |)`, `JSON_QUERY(... RETURNING |)`,
    /// etc. (Oracle and MySQL both accept a data type here, the same grammar as
    /// `CAST(... AS type)`.) Anchored on the innermost open paren following a
    /// `JSON_*` function so a statement-level DML `RETURNING <col>` — which lists
    /// columns, not types, and is never inside a function call — is left as a
    /// column position by the phase machine.
    fn cursor_is_at_json_returning_type(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(last) = toks.len().checked_sub(1) else {
            return false;
        };
        if !matches!(toks.get(last), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("RETURNING"))
        {
            return false;
        }
        Self::innermost_open_paren_preceding_word(tokens, end)
            .is_some_and(|word| word.to_ascii_uppercase().starts_with("JSON_"))
    }

    /// True when the cursor is at a column-type slot inside a `JSON_TABLE`/
    /// `XMLTABLE` `COLUMNS` clause — `COLUMNS (id | PATH …)` or `COLUMNS id |`.
    /// Scoped to the table function so an ordinary table named `columns` with an
    /// alias never triggers it.
    fn cursor_is_in_table_function_columns_type(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        if toks.len() < 2 {
            return false;
        }
        let last = toks.len() - 1;
        // The cursor must follow a plain column-name identifier.
        if !matches!(toks.get(last), Some(SqlToken::Word(word)) if !Self::is_ddl_structural_keyword(word))
        {
            return false;
        }
        let anchored = matches!(toks.get(last - 1), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("COLUMNS"))
            || matches!(toks.get(last - 1), Some(SqlToken::Symbol(sym)) if sym == "(" || sym == ",");
        if !anchored {
            return false;
        }

        Self::cursor_is_inside_table_function_columns_clause(tokens, end)
    }

    /// True when the cursor is inside a `JSON_TABLE`/`XMLTABLE` call after its
    /// `COLUMNS` keyword. Shared by the type-slot and string-literal-slot checks
    /// so the table-function subgrammar stays anchored in one place.
    fn cursor_is_inside_table_function_columns_clause(tokens: &[SqlToken], end: usize) -> bool {
        Self::cursor_is_inside_table_function_columns_clause_matching(tokens, end, |word| {
            word.eq_ignore_ascii_case("JSON_TABLE") || word.eq_ignore_ascii_case("XMLTABLE")
        })
    }

    fn cursor_is_inside_table_function_columns_clause_matching<F>(
        tokens: &[SqlToken],
        end: usize,
        matches_table_function: F,
    ) -> bool
    where
        F: Fn(&str) -> bool,
    {
        struct Frame {
            follows_table_fn: bool,
            columns_seen: bool,
        }
        let mut stack: Vec<Frame> = Vec::new();
        let mut last_word: Option<String> = None;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    let follows_table_fn = last_word.as_deref().is_some_and(|word| {
                        matches_table_function(word)
                    });
                    stack.push(Frame {
                        follows_table_fn,
                        columns_seen: false,
                    });
                    last_word = None;
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    stack.pop();
                    last_word = None;
                }
                SqlToken::Word(word) => {
                    if word.eq_ignore_ascii_case("COLUMNS") {
                        if let Some(top) = stack.last_mut() {
                            if top.follows_table_fn {
                                top.columns_seen = true;
                            }
                        }
                    }
                    last_word = Some(word.clone());
                }
                _ => last_word = None,
            }
        }
        stack
            .iter()
            .any(|frame| frame.follows_table_fn && frame.columns_seen)
    }

    /// True when the cursor follows a `PATH` keyword inside a `JSON_TABLE`/
    /// `XMLTABLE` `COLUMNS` clause. The next token must be a path string literal,
    /// not a relation, column, or type. A column literally named `path` remains a
    /// type slot because it is preceded by `COLUMNS`, `(`, or `,`.
    fn table_function_path_literal_position(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(last) = toks.len().checked_sub(1) else {
            return false;
        };
        if !matches!(toks.get(last), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("PATH"))
            || matches!(
                toks.get(last.checked_sub(1).unwrap_or(last)),
                Some(SqlToken::Symbol(sym)) if sym == "."
            )
        {
            return false;
        }
        match toks.get(last.checked_sub(1).unwrap_or(last)) {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("COLUMNS") => return false,
            Some(SqlToken::Symbol(sym)) if sym == "(" || sym == "," => return false,
            _ => {}
        }
        Self::cursor_is_inside_table_function_columns_clause(tokens, end)
    }

    fn table_function_path_literal_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::table_function_path_literal_position(tokens, end)
    }

    fn json_default_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        let mut paren_depth = 0usize;
        for token in toks[..on_index].iter().rev() {
            match token {
                SqlToken::Symbol(sym) if sym == ")" => paren_depth += 1,
                SqlToken::Symbol(sym) if sym == "(" => {
                    if paren_depth == 0 {
                        return false;
                    }
                    paren_depth -= 1;
                }
                SqlToken::Symbol(sym) if sym == "," && paren_depth == 0 => return false,
                SqlToken::Word(word)
                    if paren_depth == 0 && word.eq_ignore_ascii_case("DEFAULT") =>
                {
                    return true;
                }
                SqlToken::Word(word)
                    if paren_depth == 0
                        && matches!(
                            word.to_ascii_uppercase().as_str(),
                            "COLUMNS"
                                | "EMPTY"
                                | "ERROR"
                                | "FALSE"
                                | "FORMAT"
                                | "NULL"
                                | "ON"
                                | "PASSING"
                                | "PATH"
                                | "RETURNING"
                                | "TRUE"
                        ) =>
                {
                    return false;
                }
                _ => {}
            }
        }
        false
    }

    fn json_query_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        let Some(prev_index) = on_index.checked_sub(1) else {
            return false;
        };
        let Some(SqlToken::Word(prev)) = toks.get(prev_index) else {
            return false;
        };
        match prev.to_ascii_uppercase().as_str() {
            "NULL" | "ERROR" | "EMPTY" => true,
            "ARRAY" | "OBJECT" => matches!(
                prev_index.checked_sub(1).and_then(|index| toks.get(index)),
                Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("EMPTY")
            ),
            _ => false,
        }
    }

    fn json_value_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        matches!(
            on_index.checked_sub(1).and_then(|index| toks.get(index)),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("NULL") || word.eq_ignore_ascii_case("ERROR")
        ) || Self::json_default_handler_precedes_on(toks, on_index)
    }

    fn json_exists_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        matches!(
            on_index.checked_sub(1).and_then(|index| toks.get(index)),
            Some(SqlToken::Word(word))
                if matches!(word.to_ascii_uppercase().as_str(), "ERROR" | "FALSE" | "TRUE")
        )
    }

    fn json_table_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        Self::json_value_handler_precedes_on(toks, on_index)
            || Self::json_query_handler_precedes_on(toks, on_index)
            || Self::json_exists_handler_precedes_on(toks, on_index)
    }

    fn json_on_null_handler_precedes_on(toks: &[&SqlToken], on_index: usize) -> bool {
        matches!(
            on_index.checked_sub(1).and_then(|index| toks.get(index)),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("ABSENT") || word.eq_ignore_ascii_case("NULL")
        )
    }

    /// Fixed continuation after the `ON` in SQL/JSON error/empty handlers:
    /// `NULL ON |`, `DEFAULT expr ON |`, `TRUE ON |`, `EMPTY ARRAY ON |`.
    /// Also covers JSON generation `ABSENT/NULL ON | -> NULL`. Scoped to JSON
    /// function/table-function syntax so ordinary `JOIN ... ON |` remains a
    /// column-expression slot.
    fn expected_json_on_target_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Option<&'static [&'static str]> {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(last) = toks.len().checked_sub(1) else {
            return None;
        };
        if !matches!(toks.get(last), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("ON"))
            || matches!(
                toks.get(last.checked_sub(1).unwrap_or(last)),
                Some(SqlToken::Symbol(sym)) if sym == "."
            )
        {
            return None;
        }

        if Self::cursor_is_inside_table_function_columns_clause_matching(
            tokens,
            end,
            |word| word.eq_ignore_ascii_case("JSON_TABLE"),
        ) && Self::json_table_handler_precedes_on(&toks, last)
        {
            return Some(JSON_ERROR_EMPTY_TARGET_WORDS);
        }

        let function_word = Self::innermost_open_paren_preceding_word(tokens, end)?;
        let upper = function_word.to_ascii_uppercase();
        if JSON_ERROR_EMPTY_OPTION_FUNCTION_WORDS.contains(&upper.as_str()) {
            let handler_matches = match upper.as_str() {
                "JSON_EXISTS" => Self::json_exists_handler_precedes_on(&toks, last),
                "JSON_QUERY" => Self::json_query_handler_precedes_on(&toks, last),
                "JSON_TABLE" | "JSON_VALUE" => Self::json_table_handler_precedes_on(&toks, last),
                _ => false,
            };
            if handler_matches {
                return Some(JSON_ERROR_EMPTY_TARGET_WORDS);
            }
        }

        if !crate::sql_text::mysql_compatibility_for_sql("", db_type)
            && JSON_ON_NULL_OPTION_FUNCTION_WORDS.contains(&upper.as_str())
            && Self::json_on_null_handler_precedes_on(&toks, last)
        {
            return Some(JSON_NULL_TARGET_WORDS);
        }
        None
    }

    fn expected_json_on_target_keyword_candidates_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Option<&'static [&'static str]> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_json_on_target_keyword_candidates(tokens, end, db_type)
    }

    /// True when the cursor is at a PL/SQL type slot: a routine parameter type
    /// (`PROCEDURE p(x |)`, `(x IN |)`), a function `RETURN |` type, a collection
    /// element type (`TABLE OF |`), or a variable declaration type in a
    /// declaration region (`DECLARE v |`, `…; w |`).
    fn cursor_is_at_plsql_type(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        if toks.len() < 2 {
            return false;
        }
        let last = toks.len() - 1;
        let is_word = |idx: usize, kw: &str| {
            matches!(toks.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case(kw))
        };
        let is_symbol = |idx: usize, sym: &str| {
            matches!(toks.get(idx), Some(SqlToken::Symbol(value)) if value == sym)
        };
        let is_plain_identifier = |idx: usize| {
            matches!(toks.get(idx), Some(SqlToken::Word(word)) if !Self::is_plsql_non_type_keyword(word))
        };

        // Function/cursor `RETURN |` signature type — a `RETURN <expr>` statement
        // lives inside the body, so it is excluded by the routine-header check
        // (`REF CURSOR RETURN |` in a type declaration is matched separately).
        if is_word(last, "RETURN")
            && (is_word(last - 1, "CURSOR") || Self::cursor_is_in_routine_header(tokens, end))
        {
            return true;
        }
        // Collection element type — `TABLE OF |`, `VARRAY(n) OF |`. Anchored on
        // `TABLE` or a declaration region so SQL's `FOR UPDATE OF <col>` (a column
        // position) never offers types.
        if is_word(last, "OF")
            && (is_word(last - 1, "TABLE")
                || Self::cursor_is_in_plsql_declaration_region(tokens, end))
        {
            return true;
        }

        // Routine parameter type, when the cursor is inside a routine's parameter
        // parentheses.
        if Self::cursor_is_inside_routine_param_list(tokens, end) {
            // After a parameter mode (`IN`, `OUT`, `IN OUT`, `NOCOPY`).
            if is_word(last, "IN") || is_word(last, "OUT") || is_word(last, "NOCOPY") {
                return true;
            }
            // Right after the parameter name that begins a parameter.
            if is_plain_identifier(last) && (is_symbol(last - 1, "(") || is_symbol(last - 1, ",")) {
                return true;
            }
        }

        // Variable declaration type inside a declaration region — the var name is
        // a plain identifier that starts a declaration (`DECLARE`, after `;`, or
        // after a routine `IS`/`AS`), or `name CONSTANT |`.
        if Self::cursor_is_in_plsql_declaration_region(tokens, end) {
            if is_word(last, "CONSTANT") {
                return true;
            }
            if is_plain_identifier(last)
                && (is_symbol(last - 1, ";")
                    || is_word(last - 1, "DECLARE")
                    || is_word(last - 1, "IS")
                    || is_word(last - 1, "AS"))
            {
                return true;
            }
        }

        false
    }

    /// Keywords that occupy an identifier slot in PL/SQL declarations but are not
    /// a variable name, so a following cursor is not a declaration-type position.
    fn is_plsql_non_type_keyword(word: &str) -> bool {
        matches!(
            word.to_ascii_uppercase().as_str(),
            "CONSTANT"
                | "TYPE"
                | "SUBTYPE"
                | "CURSOR"
                | "PRAGMA"
                | "FUNCTION"
                | "PROCEDURE"
                | "BEGIN"
                | "END"
                | "DECLARE"
                | "IS"
                | "AS"
                | "EXCEPTION"
                | "RETURN"
                | "IN"
                | "OUT"
                | "NOCOPY"
                | "NOT"
                | "NULL"
                | "DEFAULT"
                | "OF"
                // SQL statement keywords can follow a cursor/routine `IS` (e.g.
                // `CURSOR c IS SELECT ...`); they are never a declared variable
                // name, so they must not be treated as a declaration-type slot.
                | "SELECT"
                | "INSERT"
                | "UPDATE"
                | "DELETE"
                | "MERGE"
                | "WITH"
                | "VALUES"
        )
    }

    /// True when the innermost still-open paren at the cursor is a routine or
    /// cursor parameter list — opened right after `FUNCTION|PROCEDURE|CURSOR name`.
    fn cursor_is_inside_routine_param_list(tokens: &[SqlToken], end: usize) -> bool {
        // Stack entry = whether this open paren is a routine/cursor param list,
        // i.e. opened right after `FUNCTION|PROCEDURE|CURSOR <name>`.
        let mut param_paren_stack: Vec<bool> = Vec::new();
        // The two most recent words seen since the last symbol.
        let mut last_word: Option<String> = None;
        let mut second_last_word: Option<String> = None;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    let name_is_identifier = last_word
                        .as_deref()
                        .is_some_and(|word| !Self::is_plsql_non_type_keyword(word));
                    let preceded_by_routine_keyword = second_last_word.as_deref().is_some_and(|word| {
                        matches!(
                            word.to_ascii_uppercase().as_str(),
                            "FUNCTION" | "PROCEDURE" | "CURSOR"
                        )
                    });
                    param_paren_stack.push(name_is_identifier && preceded_by_routine_keyword);
                    last_word = None;
                    second_last_word = None;
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    param_paren_stack.pop();
                    last_word = None;
                    second_last_word = None;
                }
                SqlToken::Word(word) => {
                    second_last_word = last_word.take();
                    last_word = Some(word.clone());
                }
                _ => {
                    last_word = None;
                    second_last_word = None;
                }
            }
        }
        param_paren_stack.last().copied().unwrap_or(false)
    }

    /// True when the cursor is in a routine signature/header — after a
    /// `FUNCTION`/`PROCEDURE` keyword and before that routine's `IS`/`AS`/`BEGIN`.
    /// Distinguishes a function-signature `RETURN type` from a body `RETURN expr`.
    fn cursor_is_in_routine_header(tokens: &[SqlToken], end: usize) -> bool {
        let mut in_header = false;
        for token in tokens.get(..end).unwrap_or(tokens) {
            if let SqlToken::Word(word) = token {
                match word.to_ascii_uppercase().as_str() {
                    "FUNCTION" | "PROCEDURE" => in_header = true,
                    "IS" | "AS" | "BEGIN" => in_header = false,
                    _ => {}
                }
            }
        }
        in_header
    }

    /// True when the cursor sits in a PL/SQL declaration region — after `DECLARE`
    /// or a routine `IS`/`AS` header and before that block's `BEGIN`.
    fn cursor_is_in_plsql_declaration_region(tokens: &[SqlToken], end: usize) -> bool {
        // Stack entry = whether this block frame is still in its declaration phase.
        let mut block_stack: Vec<bool> = Vec::new();
        let mut pending_routine_header = false;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    match upper.as_str() {
                        "FUNCTION" | "PROCEDURE" | "PACKAGE" => pending_routine_header = true,
                        "DECLARE" => block_stack.push(true),
                        "IS" | "AS" => {
                            if pending_routine_header {
                                block_stack.push(true);
                                pending_routine_header = false;
                            }
                        }
                        "BEGIN" => {
                            match block_stack.last_mut() {
                                Some(top) if *top => *top = false,
                                _ => block_stack.push(false),
                            }
                            pending_routine_header = false;
                        }
                        "END" => {
                            block_stack.pop();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        matches!(block_stack.last(), Some(true))
    }

    /// True when the cursor is right after the `AS` of a `CAST`/`TREAT`/`XMLCAST`
    /// call — `CAST(expr AS |)`. Anchored on the enclosing function so precision
    /// args (`CAST(x AS NUMBER(|))`) and ordinary `AS` aliases never trigger it.
    fn cursor_is_after_cast_as(tokens: &[SqlToken], end: usize) -> bool {
        // Stack entry = whether this open paren follows an `…(expr AS type)`
        // function: CAST/TREAT/XMLCAST, plus XMLSERIALIZE (`XMLSERIALIZE(DOCUMENT
        // x AS |)`) and VALIDATE_CONVERSION (`VALIDATE_CONVERSION(x AS |)`), which
        // share the same `AS <type>` slot.
        let mut cast_paren_stack: Vec<bool> = Vec::new();
        let mut last_word: Option<&str> = None;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    let follows_cast = last_word.is_some_and(|word| {
                        word.eq_ignore_ascii_case("CAST")
                            || word.eq_ignore_ascii_case("TREAT")
                            || word.eq_ignore_ascii_case("XMLCAST")
                            || word.eq_ignore_ascii_case("XMLSERIALIZE")
                            || word.eq_ignore_ascii_case("VALIDATE_CONVERSION")
                    });
                    cast_paren_stack.push(follows_cast);
                    last_word = None;
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    cast_paren_stack.pop();
                    last_word = None;
                }
                SqlToken::Word(word) => last_word = Some(word),
                _ => last_word = None,
            }
        }
        cast_paren_stack.last().copied().unwrap_or(false)
            && last_word.is_some_and(|word| word.eq_ignore_ascii_case("AS"))
    }

    /// The `EXTRACT(<field> FROM <source>)` argument slot at the cursor, if any.
    /// The field slot accepts only a datetime field keyword (`YEAR`, `MONTH`, …),
    /// never a column, so columns are suppressed and the dialect's field keywords
    /// are offered there. Anchored on the innermost open paren following `EXTRACT`
    /// and on `FROM` not yet appearing in that paren, so the source expression
    /// (`EXTRACT(YEAR FROM |)`, a real column position) is left untouched.
    fn extract_field_position(tokens: &[SqlToken], end: usize) -> Option<ExtractArgPosition> {
        struct Frame {
            follows_extract: bool,
            seen_from: bool,
        }
        let mut stack: Vec<Frame> = Vec::new();
        // Whether the token immediately before the cursor (within the open paren)
        // is the paren itself — i.e. no field word has been typed yet.
        let mut last_was_open_paren = false;
        let mut last_word: Option<&str> = None;
        for token in tokens.get(..end).unwrap_or(tokens) {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    let follows_extract =
                        last_word.is_some_and(|word| word.eq_ignore_ascii_case("EXTRACT"));
                    stack.push(Frame {
                        follows_extract,
                        seen_from: false,
                    });
                    last_word = None;
                    last_was_open_paren = true;
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    stack.pop();
                    last_word = None;
                    last_was_open_paren = false;
                }
                SqlToken::Word(word) => {
                    if word.eq_ignore_ascii_case("FROM") {
                        if let Some(top) = stack.last_mut() {
                            top.seen_from = true;
                        }
                    }
                    last_word = Some(word);
                    last_was_open_paren = false;
                }
                _ => {
                    last_word = None;
                    last_was_open_paren = false;
                }
            }
        }
        match stack.last() {
            Some(frame) if frame.follows_extract && !frame.seen_from => Some(if last_was_open_paren {
                ExtractArgPosition::Field
            } else {
                ExtractArgPosition::AwaitingFrom
            }),
            _ => None,
        }
    }

    fn extract_field_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<ExtractArgPosition> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::extract_field_position(tokens, end)
    }

    fn is_interval_unit_word(word: &str) -> bool {
        matches!(
            word.to_ascii_uppercase().as_str(),
            "YEAR" | "MONTH" | "DAY" | "HOUR" | "MINUTE" | "SECOND"
        )
    }

    /// The interval-literal qualifier slot at the cursor, if any. An ANSI/Oracle
    /// `INTERVAL '<value>' <unit> [TO <unit>]` literal carries its value inside
    /// the string, so the qualifier that follows is keyword-only — a column is
    /// never valid. Anchored on `INTERVAL` immediately followed by a string
    /// literal, so MySQL's unquoted `INTERVAL <expr> <unit>` (where `<expr>` may
    /// be a column) is deliberately left untouched. The rare precision-paren form
    /// (`INTERVAL '5' DAY(2) TO …`) is not matched.
    fn interval_unit_position(tokens: &[SqlToken], end: usize) -> Option<IntervalUnitSlot> {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let last = toks.len().checked_sub(1)?;
        let is_interval = |idx: usize| {
            matches!(toks.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("INTERVAL"))
        };
        let is_string = |idx: usize| matches!(toks.get(idx), Some(SqlToken::String(_)));
        let is_unit =
            |idx: usize| matches!(toks.get(idx), Some(SqlToken::Word(word)) if Self::is_interval_unit_word(word));
        let is_to = |idx: usize| {
            matches!(toks.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("TO"))
        };

        // `INTERVAL '<value>' |` -> leading qualifier unit.
        if is_string(last) && last.checked_sub(1).is_some_and(is_interval) {
            return Some(IntervalUnitSlot::Leading);
        }
        // `INTERVAL '<value>' <unit> |` -> only `TO` (or end) follows.
        if is_unit(last)
            && last.checked_sub(1).is_some_and(is_string)
            && last.checked_sub(2).is_some_and(is_interval)
        {
            return Some(IntervalUnitSlot::AwaitingTo);
        }
        // `INTERVAL '<value>' <unit> TO |` -> trailing qualifier unit.
        if is_to(last)
            && last.checked_sub(1).is_some_and(is_unit)
            && last.checked_sub(2).is_some_and(is_string)
            && last.checked_sub(3).is_some_and(is_interval)
        {
            return Some(IntervalUnitSlot::Trailing);
        }
        None
    }

    fn interval_unit_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<IntervalUnitSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::interval_unit_position(tokens, end)
    }

    /// True when the cursor sits immediately after a typed temporal-literal
    /// introducer whose next token must be the literal body, not an identifier:
    /// `DATE |`, `TIME |`, `TIMESTAMP |`; and Oracle/ANSI `INTERVAL |`.
    /// MySQL's unquoted `INTERVAL <expr> <unit>` deliberately stays an expression
    /// slot, so it is not suppressed for MySQL-compatible dialects.
    fn temporal_literal_body_position(
        tokens: &[SqlToken],
        end: usize,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(SqlToken::Word(word)) = toks.last() else {
            return false;
        };
        if matches!(
            toks.get(toks.len().saturating_sub(2)),
            Some(SqlToken::Symbol(sym)) if sym == "."
        ) {
            return false;
        }
        match word.to_ascii_uppercase().as_str() {
            "DATE" | "TIME" | "TIMESTAMP" => true,
            "INTERVAL" => !crate::sql_text::mysql_compatibility_for_sql("", db_type),
            _ => false,
        }
    }

    fn temporal_literal_body_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::temporal_literal_body_position(tokens, end, db_type)
    }

    /// True when a value-expression construct keyword has been written and its
    /// next token must be `(`, not an identifier: `CAST |`, `EXISTS |`,
    /// `XMLCAST |`, and Oracle `CURSOR |`. The phase gate keeps DDL/name slots
    /// such as `CREATE TABLE IF NOT EXISTS |` out of this value-only suppression
    /// path.
    fn expression_construct_open_paren_position(
        tokens: &[SqlToken],
        end: usize,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(SqlToken::Word(word)) = toks.last() else {
            return false;
        };
        if matches!(
            toks.get(toks.len().saturating_sub(2)),
            Some(SqlToken::Symbol(sym)) if sym == "."
        ) {
            return false;
        }
        let upper = word.to_ascii_uppercase();
        if upper == "CURSOR" {
            return !crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        PARENTHESIZED_EXPRESSION_CONSTRUCT_WORDS.contains(&upper.as_str())
    }

    fn phase_allows_expression_construct_open_paren(
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        matches!(
            phase,
            intellisense_context::SqlPhase::SelectList
                | intellisense_context::SqlPhase::JoinCondition
                | intellisense_context::SqlPhase::WhereClause
                | intellisense_context::SqlPhase::GroupByClause
                | intellisense_context::SqlPhase::HavingClause
                | intellisense_context::SqlPhase::OrderByClause
                | intellisense_context::SqlPhase::SetClause
                | intellisense_context::SqlPhase::DmlReturningList
                | intellisense_context::SqlPhase::ConnectByClause
                | intellisense_context::SqlPhase::StartWithClause
                | intellisense_context::SqlPhase::MatchRecognizeClause
                | intellisense_context::SqlPhase::ValuesClause
                | intellisense_context::SqlPhase::PivotClause
                | intellisense_context::SqlPhase::ModelClause
        )
    }

    fn expression_construct_open_paren_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        if !Self::phase_allows_expression_construct_open_paren(deep_ctx.phase) {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expression_construct_open_paren_position(tokens, end, db_type)
    }

    /// True when a table-source construct keyword has been written in a `FROM`
    /// clause and its next token must be `(`, not another relation name:
    /// `JSON_TABLE |`, Oracle `XMLTABLE |`, and Oracle `TABLE |`.
    fn table_source_construct_open_paren_position(
        tokens: &[SqlToken],
        end: usize,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(SqlToken::Word(word)) = toks.last() else {
            return false;
        };
        if matches!(
            toks.get(toks.len().saturating_sub(2)),
            Some(SqlToken::Symbol(sym)) if sym == "."
        ) {
            return false;
        }
        let upper = word.to_ascii_uppercase();
        if !PARENTHESIZED_TABLE_SOURCE_CONSTRUCT_WORDS.contains(&upper.as_str()) {
            return false;
        }
        if matches!(upper.as_str(), "TABLE" | "XMLTABLE") {
            return !crate::sql_text::mysql_compatibility_for_sql("", db_type);
        }
        true
    }

    fn table_source_construct_open_paren_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        if !matches!(deep_ctx.phase, intellisense_context::SqlPhase::FromClause) {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::table_source_construct_open_paren_position(tokens, end, db_type)
    }

    /// The single enumeration of cursor positions whose grammar is keyword- or
    /// value-only, where a column is never valid and must be suppressed. Every
    /// keyword-only slot here has a matching keyword hint in
    /// `collect_expected_keyword_suggestions`; value-only slots may emit no
    /// keyword at all. Keeping the list in one predicate is what prevents column
    /// suppression and keyword emission from drifting apart as new slots are added.
    /// ORDER BY sort
    /// modifier tails (`ASC|DESC |`, `NULLS |`, `NULLS FIRST|LAST |`) are included
    /// because the next sort key requires a comma; a bare operand is not
    /// grammatical there. Note window frames contribute only their fixed-keyword
    /// and completed-tail slots (`UNBOUNDED |`, `CURRENT |`, complete bounds,
    /// `EXCLUDE |`); value-bound slots (`ROWS |`, `BETWEEN |`, `AND |`) still
    /// accept an expression, so they emit keywords without suppressing columns
    /// and are intentionally absent here.
    #[cfg(test)]
    fn cursor_is_at_column_suppressing_keyword_slot(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        Self::cursor_is_at_column_suppressing_keyword_slot_for_db(
            deep_ctx,
            exclude_current_identifier_chain,
            None,
        )
    }

    fn cursor_is_at_column_suppressing_keyword_slot_for_db(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> bool {
        Self::data_type_position_for_context(deep_ctx, exclude_current_identifier_chain).is_some()
            || Self::cursor_is_in_row_limiting_clause_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_window_frame_keyword_only_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_window_spec_start_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::keep_dense_rank_slot_for_context(deep_ctx, exclude_current_identifier_chain)
                .is_some()
            || Self::within_group_slot_for_context(deep_ctx, exclude_current_identifier_chain)
                .is_some()
            || Self::analytic_null_treatment_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            .is_some_and(analytic_null_treatment_slot_suppresses_columns)
            || Self::cursor_is_at_type_attribute_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::extract_field_position_for_context(deep_ctx, exclude_current_identifier_chain)
                .is_some()
            || Self::interval_unit_position_for_context(deep_ctx, exclude_current_identifier_chain)
                .is_some()
            || Self::temporal_literal_body_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
                db_type,
            )
            || Self::expression_construct_open_paren_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
                db_type,
            )
            || Self::table_source_construct_open_paren_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
                db_type,
            )
            || Self::table_function_path_literal_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::expected_json_on_target_keyword_candidates_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
                db_type,
            )
            .is_some()
            || Self::order_by_sort_modifier_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            .is_some()
            || Self::window_order_by_sort_modifier_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            .is_some()
            || Self::expected_window_spec_clause_transition_candidates_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            .is_some()
            || Self::cursor_is_at_pure_clause_keyword_continuation_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_is_null_test_keyword_position_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_merge_then_action_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_merge_when_keyword_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_after_set_operator_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_after_complete_from_relation_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_after_complete_alter_table_target_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_at_locking_clause_keyword_slot_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::cursor_is_in_table_sample_clause_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
            )
            || Self::expected_sounds_like_keyword_candidates_for_context(
                deep_ctx,
                exclude_current_identifier_chain,
                db_type,
            )
            .is_some()
    }

    /// True when the cursor is inside an Oracle `TABLESAMPLE`/row-sampling clause
    /// value slot — `FROM t SAMPLE (|)`, `FROM t SAMPLE BLOCK (|)`, or the
    /// `... SAMPLE (n) SEED (|)` seed slot. These accept only a numeric sampling
    /// percentage / seed, never a relation or column, but the cursor is still in
    /// the `FROM` table phase, so the relation list would otherwise leak there.
    /// Gated on a table context and on the enclosing paren's introducer word so a
    /// same-named function or column elsewhere is untouched; the slot is
    /// value-only, so no keyword is emitted (the popup simply stays empty).
    fn cursor_is_in_table_sample_clause(tokens: &[SqlToken], end: usize) -> bool {
        Self::innermost_open_paren_preceding_word(tokens, end).is_some_and(|word| {
            matches!(word.to_ascii_uppercase().as_str(), "SAMPLE" | "SEED" | "BLOCK")
        })
    }

    fn cursor_is_in_table_sample_clause_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        if !deep_ctx.phase.is_table_context() {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::cursor_is_in_table_sample_clause(tokens, end)
    }

    /// The fixed modifier tail after an `ORDER BY` sort key. Once a direction,
    /// `NULLS`, or `NULLS FIRST|LAST` modifier has been written, another bare
    /// operand is no longer grammatical until a comma starts the next key; only
    /// the next modifier keyword, if any, can appear at the cursor. Scoped to
    /// `OrderByClause` so lookalikes such as `CREATE INDEX ... (col ASC |)` keep
    /// their own grammar, and qualified members (`t.nulls |`) stay column
    /// references.
    fn order_by_sort_modifier_slot(
        tokens: &[SqlToken],
        end: usize,
        phase: intellisense_context::SqlPhase,
    ) -> Option<OrderBySortModifierSlot> {
        if !matches!(phase, intellisense_context::SqlPhase::OrderByClause)
            || Self::trigger_word_is_qualified_member(tokens, end)
        {
            return None;
        }
        match Self::previous_meaningful_words_upper(tokens, end, 3).as_slice() {
            [.., last] if matches!(last.as_str(), "ASC" | "DESC") => {
                Some(OrderBySortModifierSlot::AfterDirection)
            }
            [.., last] if *last == "NULLS" => Some(OrderBySortModifierSlot::AfterNulls),
            [.., prev, last]
                if *prev == "NULLS" && matches!(last.as_str(), "FIRST" | "LAST") =>
            {
                Some(OrderBySortModifierSlot::AfterNullOrdering)
            }
            _ => None,
        }
    }

    fn order_by_sort_modifier_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<OrderBySortModifierSlot> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::order_by_sort_modifier_slot(tokens, end, deep_ctx.phase)
    }

    /// True when the cursor sits at a "pure clause-keyword continuation" slot:
    /// immediately after a clause-starter keyword whose only grammatical
    /// continuation is another fixed keyword, never an identifier. These are the
    /// multi-word clause openers whose first word the phase machine cannot yet
    /// resolve (the trailing `BY`/`WITH`/`JOIN` has not been typed), so the
    /// cursor is left in the surrounding table/column phase and would otherwise
    /// offer every relation or column there:
    ///
    ///   * `ORDER |` / `GROUP |` / `CONNECT |`           → `BY`
    ///   * `PARTITION |` (inside an `OVER`/`WINDOW` spec) → `BY`
    ///   * `ORDER SIBLINGS |`                            → `BY`
    ///   * `START |`                                     → `WITH`
    ///   * `LEFT|RIGHT|FULL|INNER|CROSS|NATURAL |`       → `JOIN`
    ///   * `LEFT|RIGHT|FULL OUTER |`                     → `JOIN`
    ///
    /// Mirrors the keyword hints `collect_expected_keyword_suggestions` emits for
    /// the same slots, so identifier suppression cannot drift from keyword
    /// emission. The join-type slots are gated on a table context (matching the
    /// emission side) so a column or function named `left`/`right`/… in an
    /// expression keeps its suggestions; the clause openers are reserved words
    /// whose bare unqualified use is only a clause start, so they suppress in any
    /// context (a qualified member like `t.order ` is excluded as a column).
    fn cursor_is_at_pure_clause_keyword_continuation_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        let words = Self::previous_meaningful_words_upper(tokens, end, 6);
        let trigger_is_qualified_member = Self::trigger_word_is_qualified_member(tokens, end);
        let in_table_context = deep_ctx.phase.is_table_context();
        match words.as_slice() {
            [.., last]
                if !trigger_is_qualified_member
                    && (*last == "ORDER" || *last == "GROUP" || *last == "CONNECT") =>
            {
                true
            }
            // `PARTITION |` only continues to `BY` inside an analytic window spec
            // (`OVER (...)` / `WINDOW name AS (...)`); elsewhere `PARTITION` takes a
            // partition name, so suppression stays scoped to the window context to
            // match the keyword-emission side.
            [.., last]
                if !trigger_is_qualified_member
                    && *last == "PARTITION"
                    && Self::cursor_is_inside_window_spec(tokens, end) =>
            {
                true
            }
            [.., prev, last] if *prev == "ORDER" && *last == "SIBLINGS" => true,
            [.., last] if !trigger_is_qualified_member && *last == "START" => true,
            // `... REFERENCES t (...) ON DELETE |` / `ON UPDATE |` -> a fixed
            // referential-action keyword (`CASCADE`, `SET NULL`, …), never a
            // relation. Anchored on the `ON` immediately before so a DML
            // `DELETE`/`UPDATE` statement keyword (not preceded by `ON`) is left
            // alone.
            [.., prev, last]
                if *prev == "ON" && matches!(last.as_str(), "DELETE" | "UPDATE") =>
            {
                true
            }
            [.., last]
                if in_table_context
                    && matches!(
                        last.as_str(),
                        "LEFT" | "RIGHT" | "FULL" | "INNER" | "CROSS" | "NATURAL"
                    ) =>
            {
                true
            }
            [.., prev, last]
                if in_table_context
                    && *last == "OUTER"
                    && matches!(prev.as_str(), "LEFT" | "RIGHT" | "FULL") =>
            {
                true
            }
            _ => false,
        }
    }

    /// True when the cursor sits right after the `IS` null-test operator —
    /// `<expr> IS |` or `<expr> IS NOT |`. Only keywords (`NOT`, `NULL`, …) are
    /// grammatical there; a bare identifier is never valid in any dialect or
    /// clause, so this is a pure keyword position regardless of phase. Mirrors the
    /// hints `collect_expected_keyword_suggestions` emits for the same slots so
    /// identifier suppression cannot drift from keyword emission. A qualified
    /// member written `t.is ` is excluded (there `is` is a column, not the
    /// operator).
    fn cursor_is_at_is_null_test_keyword_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        let words = Self::previous_meaningful_words_upper(tokens, end, 6);
        let trigger_is_qualified_member = Self::trigger_word_is_qualified_member(tokens, end);
        match words.as_slice() {
            [.., prev, last] if *prev == "IS" && *last == "NOT" => true,
            [.., last] if !trigger_is_qualified_member && *last == "IS" => true,
            _ => false,
        }
    }

    /// Structural keyword hint for the position right after a *complete* DML
    /// target table, where the phase machine still reports a table-target phase
    /// but a bare relation is no longer grammatical — only the clause keyword that
    /// follows the target is:
    ///
    ///   * `UPDATE t [alias] |`          → `SET`
    ///   * `DELETE FROM t [alias] |`     → `WHERE`
    ///   * `INSERT INTO t |`             → `VALUES` / `SELECT`
    ///   * `MERGE INTO t [alias] |`      → `USING`
    ///
    /// Returns `None` while the target is still being typed (the cursor's word is
    /// excluded by the caller, leaving the leading keyword as the last token) so
    /// table completion keeps working there. `UPDATE` is recognized by its
    /// dedicated `UpdateTarget` phase, which excludes post-`JOIN` positions
    /// (those become `FromClause`); `DELETE` shares `FromClause` with `SELECT`, so
    /// it is keyed on the leading `DELETE` and limited to the single-table form
    /// (no `JOIN`/comma) to avoid a MySQL multi-table delete's join targets.
    fn expected_dml_target_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
        phase: intellisense_context::SqlPhase,
    ) -> Option<&'static [&'static str]> {
        use intellisense_context::SqlPhase;

        let toks = Self::meaningful_tokens_before(tokens, end);
        // The target (or its alias) must be complete: the last token is a plain
        // identifier word, not a leading/connecting keyword and not a separator.
        let SqlToken::Word(last) = toks.last()? else {
            return None;
        };
        if matches!(
            last.to_ascii_uppercase().as_str(),
            "UPDATE" | "DELETE" | "INSERT" | "MERGE" | "INTO" | "FROM" | "USING" | "SET" | "VALUES"
        ) {
            return None;
        }

        let lead = toks.iter().find_map(|token| match token {
            SqlToken::Word(word) => Some(word.to_ascii_uppercase()),
            _ => None,
        })?;

        match lead.as_str() {
            "UPDATE" if matches!(phase, SqlPhase::UpdateTarget) => Some(&["SET"]),
            "INSERT" if matches!(phase, SqlPhase::IntoClause) => Some(&["VALUES", "SELECT"]),
            "MERGE" if matches!(phase, SqlPhase::IntoClause | SqlPhase::MergeTarget) => {
                Some(&["USING"])
            }
            "DELETE"
                if matches!(phase, SqlPhase::FromClause | SqlPhase::DeleteTarget)
                    && !toks.iter().any(|token| {
                        matches!(token, SqlToken::Word(word) if word.eq_ignore_ascii_case("JOIN"))
                            || matches!(token, SqlToken::Symbol(sym) if sym == ",")
                    }) =>
            {
                Some(&["WHERE"])
            }
            _ => None,
        }
    }

    /// True when the cursor is right after a complete DML target table (see
    /// [`Self::expected_dml_target_keyword_candidates`]). The position expects a
    /// structural clause keyword, never another relation, so the identifier list
    /// is suppressed there.
    fn cursor_is_after_complete_dml_target_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_dml_target_keyword_candidates(tokens, end, deep_ctx.phase).is_some()
    }

    /// Join-condition keyword hint for the position right after a *complete* JOIN
    /// target table — `… JOIN t [alias] |` → `ON` / `USING`. The phase machine
    /// keeps the whole `FROM` clause in `FromClause`, so without this the slot
    /// would offer every relation even though a bare table is not grammatical
    /// after a join target (only `ON`/`USING`, an alias, a further `JOIN`, or the
    /// next clause is).
    ///
    /// Returns `None` while the target is still being typed (the cursor's word is
    /// excluded by the caller, leaving `JOIN`/the join type as the last token) so
    /// table completion keeps working there, and for `CROSS`/`NATURAL` joins,
    /// which take neither `ON` nor `USING`.
    fn expected_join_target_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
        phase: intellisense_context::SqlPhase,
    ) -> Option<&'static [&'static str]> {
        use intellisense_context::SqlPhase;

        if !matches!(phase, SqlPhase::FromClause) {
            return None;
        }
        let is_join_keyword = |word: &str| {
            matches!(
                word.to_ascii_uppercase().as_str(),
                "JOIN"
                    | "ON"
                    | "USING"
                    | "INNER"
                    | "OUTER"
                    | "LEFT"
                    | "RIGHT"
                    | "FULL"
                    | "CROSS"
                    | "NATURAL"
            )
        };

        let toks = Self::meaningful_tokens_before(tokens, end);
        // The join target (or its alias) must be complete: the last token is a
        // plain identifier word, not a join keyword and not a separator.
        let SqlToken::Word(last) = toks.last()? else {
            return None;
        };
        if is_join_keyword(last) {
            return None;
        }

        // Walk back to the governing `JOIN`; between it and the cursor only the
        // target name (`schema.table`) and an optional alias word may appear.
        let mut idx = toks.len() - 1;
        let join_idx = loop {
            if idx == 0 {
                return None;
            }
            idx -= 1;
            match &toks[idx] {
                SqlToken::Word(word) if word.eq_ignore_ascii_case("JOIN") => break idx,
                SqlToken::Word(_) => {} // part of the table name or its alias
                SqlToken::Symbol(sym) if sym == "." => {} // dotted name separator
                _ => return None, // comma / paren / operator: not a simple target
            }
        };

        // `CROSS JOIN` / `NATURAL JOIN` take no join condition.
        let mut type_idx = join_idx;
        while type_idx > 0 {
            type_idx -= 1;
            match &toks[type_idx] {
                SqlToken::Word(word)
                    if matches!(
                        word.to_ascii_uppercase().as_str(),
                        "LEFT" | "RIGHT" | "FULL" | "INNER" | "OUTER"
                    ) => {}
                SqlToken::Word(word)
                    if matches!(word.to_ascii_uppercase().as_str(), "CROSS" | "NATURAL") =>
                {
                    return None;
                }
                _ => break,
            }
        }

        Some(&["ON", "USING"])
    }

    /// True when the cursor is right after a complete JOIN target table (see
    /// [`Self::expected_join_target_keyword_candidates`]). The position expects a
    /// join condition keyword, never another relation, so the identifier list is
    /// suppressed there.
    fn cursor_is_after_complete_join_target_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_join_target_keyword_candidates(tokens, end, deep_ctx.phase).is_some()
    }

    /// True when the cursor sits right after a *complete* relation reference in a
    /// `FROM` comma-list — the first table (`FROM emp |`) or a comma-separated one
    /// (`FROM a, b |`), with or without an implicit alias (`FROM emp e |`). The
    /// phase machine keeps the whole clause in `FromClause`, so without this the
    /// slot dumps every relation even though a bare relation is not grammatical
    /// there (a second table needs a leading comma or `JOIN`; the slot itself is
    /// the implicit-alias position, a brand-new name). It is the comma-list
    /// companion of [`Self::expected_join_target_keyword_candidates`], which
    /// covers the `JOIN` side; the join run is deliberately excluded here so that
    /// predicate keeps emitting its `ON`/`USING` hint. While the relation is still
    /// being typed the caller excludes the cursor word, leaving `FROM`/`,` as the
    /// last token, so relation completion keeps working.
    fn cursor_is_after_complete_from_relation(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        // The reference (or its implicit alias) must be complete: the last token
        // is a plain identifier word, never a separator or a clause keyword.
        let Some(SqlToken::Word(last)) = toks.last() else {
            return false;
        };
        if Self::token_is_language_keyword(&last.to_ascii_uppercase()) {
            return false;
        }
        // Walk back over the reference's own words / dotted-name separators to the
        // structural token that introduces the list slot: `FROM` or a `,` means a
        // complete comma-list reference; anything else (a `JOIN`/join-type word, a
        // `(`, an operator) belongs to another slot and is left untouched.
        let mut idx = toks.len() - 1;
        while idx > 0 {
            idx -= 1;
            match &toks[idx] {
                SqlToken::Word(word) if word.eq_ignore_ascii_case("FROM") => return true,
                SqlToken::Symbol(sym) if sym == "," => return true,
                SqlToken::Word(_) => {} // part of the table name or its implicit alias
                SqlToken::Symbol(sym) if sym == "." => {} // dotted-name separator
                _ => return false,
            }
        }
        false
    }

    fn cursor_is_after_complete_from_relation_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        if !matches!(deep_ctx.phase, intellisense_context::SqlPhase::FromClause) {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::cursor_is_after_complete_from_relation(tokens, end)
    }

    /// True when the cursor sits right after the *complete* target name of an
    /// `ALTER TABLE <name> |` statement (including a qualified `schema.name`). The
    /// table name is already given, so what follows is an alteration clause
    /// (`ADD`/`MODIFY`/`DROP`/`RENAME`/…), never another relation; the phase
    /// machine leaves the cursor in the `ALTER TABLE` target phase (`IntoClause`),
    /// where the relation list would otherwise be offered a second time. Anchored
    /// on a leading `ALTER TABLE` so the name slot itself (`ALTER TABLE |`, last
    /// token `TABLE`) still completes relations, and so the `IntoClause` of an
    /// `INSERT`/`CREATE TABLE AS`/`COMMENT ON` is untouched.
    fn cursor_is_after_complete_alter_table_target(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let Some(SqlToken::Word(last)) = toks.last() else {
            return false;
        };
        if Self::token_is_language_keyword(&last.to_ascii_uppercase()) {
            return false;
        }
        // Walk back over the (possibly dotted) target name to the `TABLE` keyword,
        // which must itself be introduced by `ALTER`.
        let mut idx = toks.len() - 1;
        while idx > 0 {
            idx -= 1;
            match &toks[idx] {
                SqlToken::Word(word) if word.eq_ignore_ascii_case("TABLE") => {
                    return idx > 0
                        && matches!(
                            toks.get(idx - 1),
                            Some(SqlToken::Word(head)) if head.eq_ignore_ascii_case("ALTER")
                        );
                }
                SqlToken::Word(_) => {} // part of the target name
                SqlToken::Symbol(sym) if sym == "." => {} // dotted-name separator
                _ => return false,
            }
        }
        false
    }

    fn cursor_is_after_complete_alter_table_target_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        if !matches!(deep_ctx.phase, intellisense_context::SqlPhase::IntoClause) {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::cursor_is_after_complete_alter_table_target(tokens, end)
    }

    /// True when the cursor is at the type slot of a column definition in
    /// `CREATE TABLE (... col |)` or `ALTER TABLE ... ADD|MODIFY|CHANGE ... col |`.
    fn cursor_is_at_ddl_column_type(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        if toks.len() < 3 {
            return false;
        }
        let word_upper = |idx: usize| match toks.get(idx) {
            Some(SqlToken::Word(word)) => Some(word.to_ascii_uppercase()),
            _ => None,
        };
        let is_symbol = |idx: usize, sym: &str| {
            matches!(toks.get(idx), Some(SqlToken::Symbol(value)) if value == sym)
        };
        let any_word = |kw: &str| {
            toks.iter()
                .any(|token| matches!(token, SqlToken::Word(word) if word.eq_ignore_ascii_case(kw)))
        };

        // The cursor must follow a plain column-name identifier.
        let last = toks.len() - 1;
        let last_is_plain_identifier = matches!(toks.get(last), Some(SqlToken::Word(word)) if !Self::is_ddl_structural_keyword(word));
        if !last_is_plain_identifier {
            return false;
        }

        let starts_with = |kw: &str| word_upper(0).as_deref() == Some(kw);

        if starts_with("ALTER") && any_word("TABLE") {
            // `ADD col |`, `MODIFY col |`, `ADD|MODIFY|CHANGE COLUMN col |`,
            // `CHANGE old new |`.
            let w2 = word_upper(last - 1);
            let w3 = word_upper(last - 2);
            if matches!(w2.as_deref(), Some("ADD") | Some("MODIFY")) {
                return true;
            }
            if w2.as_deref() == Some("COLUMN")
                && matches!(w3.as_deref(), Some("ADD") | Some("MODIFY") | Some("CHANGE"))
            {
                return true;
            }
            if w3.as_deref() == Some("CHANGE") {
                return true;
            }
            // Oracle parenthesized form: `ADD (col |)`, `MODIFY (c1 NUMBER, c2 |)`.
            // The enclosing paren must directly follow `ADD`/`MODIFY` so that
            // `ADD CHECK (...)` / `ADD CONSTRAINT ... (...)` are not mistaken.
            if is_symbol(last - 1, "(") || is_symbol(last - 1, ",") {
                if let Some(word) = Self::innermost_open_paren_preceding_word(tokens, end) {
                    if matches!(word.to_ascii_uppercase().as_str(), "ADD" | "MODIFY") {
                        return true;
                    }
                }
            }
            return false;
        }

        if starts_with("CREATE") && any_word("TABLE") && !any_word("SELECT") {
            // Inside the column-definition list, a column name begins after `(` or
            // `,`. Exclude CTAS column lists (handled by the `SELECT` guard above).
            if is_symbol(last - 1, "(") || is_symbol(last - 1, ",") {
                let open_parens = toks
                    .iter()
                    .filter(|token| matches!(token, SqlToken::Symbol(sym) if sym == "("))
                    .count();
                let close_parens = toks
                    .iter()
                    .filter(|token| matches!(token, SqlToken::Symbol(sym) if sym == ")"))
                    .count();
                return open_parens > close_parens;
            }
        }

        false
    }

    /// Whether the cursor in `deep_ctx` is at a data-type position (used to both
    /// emit type suggestions and suppress column suggestions there).
    fn data_type_position_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> Option<DataTypePosition> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::data_type_position(tokens, end)
    }

    /// True when the cursor sits right after a standalone `AS` — an alias-name
    /// slot that introduces a brand-new identifier (`expr AS |`, `relation
    /// AS |`). Such a slot is never an existing column/relation/keyword, so
    /// identifier suggestions are suppressed there, extending the typed-alias
    /// suppression in `LocalAliasContext` (which only engages once a character is
    /// typed) to the still-empty slot. Excluded for data-type slots
    /// (`CAST(x AS |)`), where `AS` introduces a type rather than an alias. The
    /// caller scopes this to the clause where `AS` actually introduces an alias
    /// (a select-list column slot, or a table/derived-table slot).
    fn cursor_word_is_alias_name_after_as(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        if Self::data_type_position_for_context(deep_ctx, exclude_current_identifier_chain)
            .is_some()
        {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::previous_meaningful_words_upper(tokens, end, 1)
            .last()
            .is_some_and(|word| word == "AS")
    }

    /// True when the cursor sits at the alias-name slot right after `AS` in a
    /// `SELECT` list (`SELECT expr AS |`). That slot names a brand-new column
    /// alias, so identifier suggestions are suppressed there. Scoped to the
    /// select-list column context, where `AS` always introduces an alias.
    fn cursor_is_at_select_list_alias_name_slot(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        matches!(sql_context_for_phase(deep_ctx.phase), SqlContext::ColumnOrAll)
            && Self::cursor_word_is_alias_name_after_as(deep_ctx, exclude_current_identifier_chain)
    }

    /// True when the cursor sits at the alias-name slot right after `AS` in a
    /// table clause (`FROM t AS |`, `FROM (subquery) AS |`, `UPDATE t AS |`,
    /// `MERGE INTO t AS |`, a JOIN target `… JOIN t AS |`). That slot names a
    /// brand-new relation alias — never an existing relation/column/keyword — so
    /// the identifier base is suppressed, mirroring the select-list alias slot
    /// for the table side. This is the empty-slot companion of the typed-alias
    /// suppression `LocalAliasContext` already applies (`FROM t AS x|`), closing
    /// the gap where the bare `FROM t AS |` slot dumped the whole relation
    /// catalog. It stays a column-suppressing slot even for Oracle's flashback
    /// `AS OF`: only the `OF` keyword or a new alias may follow `AS`, never an
    /// identifier. Routed through the keyword-only-slot family (not a hard
    /// suppress) so the clause keywords those target slots still expect —
    /// `UPDATE t AS |` → `SET`, `DELETE … t AS |` → `WHERE`, `… JOIN t AS |` →
    /// `ON`/`USING` — keep flowing from the keyword merge.
    fn cursor_is_at_table_alias_name_slot(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        matches!(sql_context_for_phase(deep_ctx.phase), SqlContext::TableName)
            && Self::cursor_word_is_alias_name_after_as(deep_ctx, exclude_current_identifier_chain)
    }

    fn cursor_is_before_current_query_from_clause(tokens: &[SqlToken], end: usize) -> bool {
        let depths = crate::ui::sql_depth::paren_depths(tokens);
        let limit = end.min(tokens.len());
        let is_top_level = |idx| crate::ui::sql_depth::is_top_level_depth(&depths, idx);

        if tokens
            .iter()
            .enumerate()
            .take(limit)
            .any(|(idx, token)| {
                is_top_level(idx)
                    && matches!(token, SqlToken::Word(word) if word.eq_ignore_ascii_case("FROM"))
            })
        {
            return false;
        }

        for (idx, token) in tokens.iter().enumerate().skip(limit) {
            if !is_top_level(idx) {
                continue;
            }
            match token {
                SqlToken::Word(word) if word.eq_ignore_ascii_case("FROM") => return true,
                SqlToken::Word(word)
                    if matches!(
                        word.to_ascii_uppercase().as_str(),
                        "UNION" | "INTERSECT" | "EXCEPT" | "MINUS"
                    ) =>
                {
                    return false;
                }
                SqlToken::Symbol(sym) if sym == ";" => return false,
                _ => {}
            }
        }
        false
    }

    /// True when the cursor is at a row-count / offset argument — `LIMIT |`,
    /// `LIMIT <offset>, |`, or `OFFSET |`. These accept an integer literal or
    /// bind only, never a column, so column suggestions are suppressed there.
    fn cursor_is_at_row_count_position(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let n = toks.len();
        if n == 0 {
            return false;
        }
        let is_word = |idx: usize, kw: &str| {
            matches!(toks.get(idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case(kw))
        };
        // `LIMIT |` / `OFFSET |`.
        if is_word(n - 1, "LIMIT") || is_word(n - 1, "OFFSET") {
            return true;
        }
        // MySQL `LIMIT <offset>, |` — the count after the comma.
        if n >= 3
            && matches!(toks.get(n - 1), Some(SqlToken::Symbol(sym)) if sym == ",")
            && is_word(n - 3, "LIMIT")
        {
            return true;
        }
        false
    }

    /// True when the cursor sits anywhere inside a row-limiting clause slot where
    /// a column is never valid. This covers both the pure value slots handled by
    /// `cursor_is_at_row_count_position` (`LIMIT |`, `OFFSET |`, `LIMIT off, |`)
    /// and every `FETCH FIRST|NEXT …` / `OFFSET <count> …` slot recognized by
    /// `expected_row_limiting_keyword_candidates` (`FETCH FIRST |`, `FETCH NEXT
    /// <count> |`, `… ROWS |`, `… PERCENT |`, `… ROWS WITH |`, `OFFSET <count>
    /// |`). The phase machine collapses all of these onto `OrderByClause`
    /// (a column context), so without this gate the row-limiting tail would
    /// wrongly offer columns alongside the row-limiting keyword hints.
    fn cursor_is_in_row_limiting_clause(
        tokens: &[SqlToken],
        end: usize,
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        if Self::cursor_is_before_current_query_from_clause(tokens, end) {
            return false;
        }
        Self::cursor_is_at_row_count_position(tokens, end)
            || Self::expected_row_limiting_keyword_candidates(tokens, end).is_some()
            || Self::cursor_is_after_complete_row_limiting_tail(tokens, end, phase)
    }

    fn cursor_is_in_row_limiting_clause_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        if Self::cursor_is_before_current_query_from_clause(tokens, end) {
            return false;
        }
        if Self::cursor_is_after_complete_row_limiting_tail(tokens, end, deep_ctx.phase) {
            return true;
        }
        if !matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause
        ) {
            return false;
        }
        Self::cursor_is_in_row_limiting_clause(tokens, end, deep_ctx.phase)
    }

    fn previous_meaningful_words_with_bind_markers_upper(
        tokens: &[SqlToken],
        end: usize,
        max_words: usize,
    ) -> Vec<(String, bool)> {
        if max_words == 0 {
            return Vec::new();
        }

        let mut words_rev = Vec::new();
        let mut idx = end.min(tokens.len());
        while idx > 0 {
            idx -= 1;
            match &tokens[idx] {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => {
                    words_rev.push((
                        word.to_ascii_uppercase(),
                        Self::word_is_preceded_by_bind_colon(tokens, idx),
                    ));
                    if words_rev.len() >= max_words {
                        break;
                    }
                }
                SqlToken::Symbol(_) => {}
                _ => break,
            }
        }
        words_rev.reverse();
        words_rev
    }

    /// True when the last meaningful token before `end` is a Word that is a
    /// qualified member — immediately preceded by `.`, e.g. the `order` in
    /// `t.order`. Such a word is unambiguously a column reference, so a dual-use
    /// clause keyword (`ORDER BY`, `GROUP BY`, `CONNECT BY`, `START WITH`) can
    /// never follow it and its continuation keyword must not be offered. (These
    /// words are reserved, so a bare unqualified use is not valid SQL; the
    /// qualified `t.order ` form — note the trailing space routes here rather than
    /// through qualified-member completion — is the only real-SQL misfire.)
    fn trigger_word_is_qualified_member(tokens: &[SqlToken], end: usize) -> bool {
        let toks = Self::meaningful_tokens_before(tokens, end);
        let last = match toks.len() {
            0 => return false,
            n => n - 1,
        };
        if last == 0 || !matches!(toks.get(last), Some(SqlToken::Word(_))) {
            return false;
        }
        matches!(toks.get(last - 1), Some(SqlToken::Symbol(sym)) if sym == ".")
    }

    fn word_is_preceded_by_bind_colon(tokens: &[SqlToken], word_idx: usize) -> bool {
        let mut idx = word_idx;
        while idx > 0 {
            idx -= 1;
            match &tokens[idx] {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(symbol) if symbol == ":" => return true,
                _ => return false,
            }
        }
        false
    }

    fn filter_expected_candidates(prefix: &str, candidates: &[&str]) -> Vec<String> {
        let prefix_upper = prefix.to_ascii_uppercase();
        let mut seen = HashSet::new();
        let mut suggestions = Vec::new();

        for candidate in candidates {
            let upper = candidate.to_ascii_uppercase();
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                suggestions.push((*candidate).to_string());
                if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                    break;
                }
            }
        }

        suggestions
    }

    fn expected_database_link_keyword_candidates(words: &[String]) -> Option<&'static [&'static str]> {
        const LINK_KEYWORDS: &[&str] = &["LINK"];
        const DATABASE_LINK_MODIFIERS: &[&str] = &["PUBLIC", "DATABASE"];
        const DATABASE_KEYWORDS: &[&str] = &["DATABASE"];

        let (allows_shared, tail) = match words {
            [first, rest @ ..] if first == "DROP" => (false, rest),
            [first, rest @ ..] if matches!(first.as_str(), "ALTER" | "CREATE") => (true, rest),
            _ => return None,
        };

        match tail {
            [database] if database == "DATABASE" => Some(LINK_KEYWORDS),
            [public, database] if public == "PUBLIC" && database == "DATABASE" => {
                Some(LINK_KEYWORDS)
            }
            [shared] if allows_shared && shared == "SHARED" => Some(DATABASE_LINK_MODIFIERS),
            [shared, database] if allows_shared && shared == "SHARED" && database == "DATABASE" => {
                Some(LINK_KEYWORDS)
            }
            [shared, public] if allows_shared && shared == "SHARED" && public == "PUBLIC" => {
                Some(DATABASE_KEYWORDS)
            }
            [shared, public, database]
                if allows_shared
                    && shared == "SHARED"
                    && public == "PUBLIC"
                    && database == "DATABASE" =>
            {
                Some(LINK_KEYWORDS)
            }
            _ => None,
        }
    }

    fn expected_java_keyword_candidates(words: &[String]) -> Option<&'static [&'static str]> {
        const JAVA_CREATE_DROP_OBJECT_TYPES: &[&str] = &["SOURCE", "CLASS", "RESOURCE"];
        const JAVA_ALTER_OBJECT_TYPES: &[&str] = &["SOURCE", "CLASS"];
        const JAVA_COMPILE_OPTIONS: &[&str] = &["COMPILE", "RESOLVE"];
        const JAVA_KEYWORD: &[&str] = &["JAVA"];
        const NAMED_KEYWORD: &[&str] = &["NAMED"];
        const USING_KEYWORD: &[&str] = &["USING"];

        match words {
            [create, or_kw, replace, and_kw]
                if *create == "CREATE"
                    && *or_kw == "OR"
                    && *replace == "REPLACE"
                    && *and_kw == "AND" =>
            {
                Some(JAVA_COMPILE_OPTIONS)
            }
            [create, or_kw, replace, and_kw, compile]
                if *create == "CREATE"
                    && *or_kw == "OR"
                    && *replace == "REPLACE"
                    && *and_kw == "AND"
                    && matches!(compile.as_str(), "COMPILE" | "RESOLVE") =>
            {
                Some(JAVA_KEYWORD)
            }
            [create, or_kw, replace, java]
                if *create == "CREATE"
                    && *or_kw == "OR"
                    && *replace == "REPLACE"
                    && *java == "JAVA" =>
            {
                Some(JAVA_CREATE_DROP_OBJECT_TYPES)
            }
            [create, or_kw, replace, and_kw, compile, java]
                if *create == "CREATE"
                    && *or_kw == "OR"
                    && *replace == "REPLACE"
                    && *and_kw == "AND"
                    && matches!(compile.as_str(), "COMPILE" | "RESOLVE")
                    && *java == "JAVA" =>
            {
                Some(JAVA_CREATE_DROP_OBJECT_TYPES)
            }
            [.., verb, java]
                if matches!(verb.as_str(), "CREATE" | "DROP") && *java == "JAVA" =>
            {
                Some(JAVA_CREATE_DROP_OBJECT_TYPES)
            }
            [.., verb, java] if *verb == "ALTER" && *java == "JAVA" => {
                Some(JAVA_ALTER_OBJECT_TYPES)
            }
            [.., java, object_type]
                if *java == "JAVA" && matches!(object_type.as_str(), "SOURCE" | "RESOURCE") =>
            {
                Some(NAMED_KEYWORD)
            }
            [.., java, object_type] if *java == "JAVA" && *object_type == "CLASS" => {
                Some(USING_KEYWORD)
            }
            _ => None,
        }
    }

    fn expected_rollback_segment_keyword_candidates(
        words: &[String],
    ) -> Option<&'static [&'static str]> {
        const SEGMENT_KEYWORD: &[&str] = &["SEGMENT"];

        match words {
            [.., verb, rollback]
                if matches!(verb.as_str(), "ALTER" | "CREATE" | "DROP")
                    && *rollback == "ROLLBACK" =>
            {
                Some(SEGMENT_KEYWORD)
            }
            [.., create, public, rollback]
                if *create == "CREATE" && *public == "PUBLIC" && *rollback == "ROLLBACK" =>
            {
                Some(SEGMENT_KEYWORD)
            }
            _ => None,
        }
    }

    fn is_create_synonym_target_context(words: &[String]) -> bool {
        if words.last().is_none_or(|word| word != "FOR") {
            return false;
        }

        let Some(synonym_idx) = words.iter().rposition(|word| word == "SYNONYM") else {
            return false;
        };
        words
            .get(..synonym_idx)
            .is_some_and(|prefix| prefix.iter().any(|word| word == "CREATE"))
    }

    fn is_create_synonym_name_written_context(words: &[String]) -> bool {
        if words.last().is_none_or(|word| word == "SYNONYM" || word == "PUBLIC") {
            return false;
        }
        if words.iter().any(|word| word == "FOR") {
            return false;
        }

        let Some(synonym_idx) = words.iter().rposition(|word| word == "SYNONYM") else {
            return false;
        };
        words.len() > synonym_idx + 1
            && words
                .get(..synonym_idx)
                .is_some_and(|prefix| prefix.iter().any(|word| word == "CREATE"))
    }

    fn is_create_on_table_target_context(words: &[String]) -> bool {
        if words.last().is_none_or(|word| word != "ON") {
            return false;
        }

        let Some(create_idx) = words.iter().rposition(|word| word == "CREATE") else {
            return false;
        };
        words
            .get(create_idx + 1..words.len().saturating_sub(1))
            .is_some_and(|middle| {
                middle
                    .iter()
                    .any(|word| matches!(word.as_str(), "INDEX" | "TRIGGER"))
            })
    }

    fn completion_suggestion_matches_prefix(suggestion: &str, prefix: &str) -> bool {
        prefix.is_empty()
            || crate::ui::intellisense::suggestion_matches_completion_prefix(suggestion, prefix)
    }

    /// True when the active query's leading keyword is `MERGE`, i.e. the cursor
    /// is inside a MERGE statement (or its merge-action clauses). Used to gate
    /// the `WHEN MATCHED`/`WHEN NOT MATCHED` keyword hints, which are MERGE-only:
    /// a `CASE WHEN |` branch in a SELECT/PL-SQL block must not offer `MATCHED`.
    fn statement_is_merge(tokens: &[SqlToken]) -> bool {
        tokens
            .iter()
            .find_map(|token| match token {
                SqlToken::Comment(_) => None,
                SqlToken::Word(word) => Some(word.eq_ignore_ascii_case("MERGE")),
                _ => Some(false),
            })
            .unwrap_or(false)
    }

    /// True when the cursor sits inside an unclosed `CASE … END` expression. A
    /// MERGE statement is pure SQL, so any `CASE` here is a value expression that
    /// closes with a bare `END` (never PL/SQL's `END CASE`/`END IF`), making a
    /// plain `CASE`/`END` balance exact. This keeps `MERGE … SET c = CASE WHEN |`
    /// from being mistaken for a `WHEN MATCHED` merge-action slot.
    fn cursor_is_inside_unclosed_case(tokens: &[SqlToken], end: usize) -> bool {
        // A bare `CASE … END` (SQL expression) and a PL/SQL `CASE … END CASE`
        // statement both make `WHEN`/`THEN`/`ELSE` grammatical, but `END` is
        // shared across every block construct: `END IF`/`END LOOP`/`END CASE`
        // each close a *different* construct, and a bare `END` closes a `BEGIN`
        // block or a SQL `CASE` expression. Naively counting every `END` as a
        // `CASE` close (and every `CASE` word as an open) miscounts twice — the
        // `CASE` in `END CASE` reopens a closed case, and the `END` in `END IF`
        // closes a still-open enclosing case. Track a block stack instead and
        // skip a qualified `END`'s keyword so it is read as one close, not a
        // close plus a new construct. (SqlParserEngine models the same block
        // structure, but over raw text lines rather than this token slice, so
        // the token-level stack mirrors its model — as `cursor_in_plsql_
        // executable_block` already does.)
        #[derive(PartialEq)]
        enum Block {
            Case,
            Other,
        }
        let toks = tokens.get(..end).unwrap_or(tokens);
        let mut stack: Vec<Block> = Vec::new();
        let mut idx = 0;
        while idx < toks.len() {
            if let SqlToken::Word(word) = &toks[idx] {
                match word.to_ascii_uppercase().as_str() {
                    "CASE" => stack.push(Block::Case),
                    "IF" | "LOOP" | "BEGIN" => stack.push(Block::Other),
                    "END" => {
                        stack.pop();
                        // A qualified `END IF`/`END LOOP`/`END CASE` consumes its
                        // keyword; skip it so the qualifier is not re-read as the
                        // start of a fresh construct.
                        if let Some((next, next_idx)) =
                            Self::next_word_upper_in_tokens(toks, idx + 1)
                        {
                            if matches!(next.as_str(), "IF" | "LOOP" | "CASE") {
                                idx = next_idx;
                            }
                        }
                    }
                    _ => {}
                }
            }
            idx += 1;
        }
        stack.contains(&Block::Case)
    }

    /// MERGE merge-action slot right after `WHEN [NOT] MATCHED [AND <cond>]
    /// THEN |`: the only grammatical continuations are `UPDATE`/`DELETE`
    /// (matched) or `INSERT` (not matched) — never a column. Returns the action
    /// keywords for the slot, or `None` when the cursor is not at it. Gated to a
    /// MERGE statement whose `THEN` is not a `CASE … THEN` branch, and robust to
    /// an `AND <condition>` between `MATCHED` and `THEN` by anchoring on the
    /// nearest preceding `WHEN`.
    fn merge_then_action_keywords(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        if !Self::statement_is_merge(tokens) || Self::cursor_is_inside_unclosed_case(tokens, end) {
            return None;
        }
        let toks = Self::meaningful_tokens_before(tokens, end);
        if !matches!(toks.last(), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("THEN")) {
            return None;
        }
        let words: Vec<String> = toks
            .iter()
            .filter_map(|token| match token {
                SqlToken::Word(word) => Some(word.to_ascii_uppercase()),
                _ => None,
            })
            .collect();
        let when_idx = words.iter().rposition(|word| word == "WHEN")?;
        match (
            words.get(when_idx + 1).map(String::as_str),
            words.get(when_idx + 2).map(String::as_str),
        ) {
            (Some("NOT"), Some("MATCHED")) => Some(&["INSERT"]),
            (Some("MATCHED"), _) => Some(&["UPDATE", "DELETE"]),
            _ => None,
        }
    }

    fn cursor_is_at_merge_then_action_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::merge_then_action_keywords(tokens, end).is_some()
    }

    /// MERGE merge-action introducer slot right after `WHEN |` (→ `MATCHED` /
    /// `NOT`) or `WHEN NOT |` (→ `MATCHED`): the only grammatical continuations
    /// are those keywords — never a column, relation, alias or local symbol. The
    /// `ON (...)` join condition that precedes the first `WHEN` leaves the cursor
    /// in `JoinCondition` (a column phase), so without this the bare `WHEN`/`WHEN
    /// NOT` slot leaked every joined column. Gated to a MERGE whose `WHEN` is not
    /// a `CASE … WHEN` branch (mirrors `merge_then_action_keywords`), and `WHEN
    /// NOT` is anchored on the preceding `WHEN` so an `IS NOT`/`AND … NOT` inside
    /// a match condition is untouched.
    fn merge_when_action_keywords(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        if !Self::statement_is_merge(tokens) || Self::cursor_is_inside_unclosed_case(tokens, end) {
            return None;
        }
        let toks = Self::meaningful_tokens_before(tokens, end);
        match toks.last() {
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("WHEN") => {
                Some(&["MATCHED", "NOT"])
            }
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("NOT") => {
                let prev = toks.len().checked_sub(2).and_then(|idx| toks.get(idx));
                match prev {
                    Some(SqlToken::Word(prev_word)) if prev_word.eq_ignore_ascii_case("WHEN") => {
                        Some(&["MATCHED"])
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn cursor_is_at_merge_when_keyword_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::merge_when_action_keywords(tokens, end).is_some()
    }

    /// The slot right after a set operator that joins two query blocks — `UNION
    /// |` / `INTERSECT |` / `EXCEPT |` / `MINUS |` (→ `SELECT` / `ALL`), and
    /// `UNION ALL |` / `UNION DISTINCT |` (→ `SELECT`). Only a new query block
    /// (or a parenthesised one) may follow, never a relation, column or function,
    /// so the slot suppresses the identifier base while these keyword hints come
    /// from the same helper — the keyword emission and column suppression cannot
    /// drift apart.
    fn expected_set_operator_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        const SET_OPERATORS: &[&str] = &["UNION", "INTERSECT", "EXCEPT", "MINUS"];
        let is_set_op =
            |word: &str| SET_OPERATORS.iter().any(|op| word.eq_ignore_ascii_case(op));
        let toks = Self::meaningful_tokens_before(tokens, end);
        match toks.last() {
            Some(SqlToken::Word(word)) if is_set_op(word) => Some(&["SELECT", "ALL"]),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("ALL") || word.eq_ignore_ascii_case("DISTINCT") =>
            {
                let prev = toks.len().checked_sub(2).and_then(|idx| toks.get(idx));
                match prev {
                    Some(SqlToken::Word(prev_word)) if is_set_op(prev_word) => Some(&["SELECT"]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn cursor_is_after_set_operator_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        Self::expected_set_operator_keyword_candidates(tokens, end).is_some()
    }

    /// Row-locking clause keyword slot — `… FOR |` (→ `UPDATE`/`SHARE`) or
    /// `… FOR UPDATE|SHARE |` (→ `OF`/`NOWAIT`/`WAIT`/`SKIP`). Only keywords are
    /// grammatical there, never a column. The caller gates this on a top-level
    /// (`depth == 0`) query column context so the `FOR` operand slots that live
    /// inside parentheses — `SUBSTRING(x FROM a FOR |)`, the `MODEL`/`PIVOT`
    /// `FOR` clauses — and PL/SQL `FOR` loops / `OPEN … FOR` (a neutral phase)
    /// keep their normal completion. `FOR UPDATE OF |` is a column list, so it
    /// returns `None` and is unaffected.
    fn expected_locking_clause_keyword_candidates(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<&'static [&'static str]> {
        let words = Self::previous_meaningful_words_upper(tokens, end, 2);
        let last = words.last().map(String::as_str);
        let prev = words
            .len()
            .checked_sub(2)
            .and_then(|idx| words.get(idx))
            .map(String::as_str);
        if prev == Some("FOR") && matches!(last, Some("UPDATE") | Some("SHARE")) {
            return Some(&["OF", "NOWAIT", "WAIT", "SKIP"]);
        }
        if last == Some("FOR") {
            return Some(&["UPDATE", "SHARE"]);
        }
        None
    }

    /// Count of `(` not yet closed by `)` in `tokens[..end]`. Used to confirm
    /// the cursor is at the statement's top level rather than inside a function
    /// call or sub-expression. Computed from the raw paren tokens because the
    /// phase machine's `depth` mis-tracks the SQL-standard `SUBSTRING(… FROM …
    /// FOR …)` / `TRIM` / `OVERLAY` syntax (the inner `FROM` is read as a query
    /// clause), which would otherwise leak into the locking-clause detection.
    fn unclosed_paren_count(tokens: &[SqlToken], end: usize) -> usize {
        let mut depth: i32 = 0;
        for token in tokens.get(..end).unwrap_or(tokens) {
            if let SqlToken::Symbol(sym) = token {
                if sym == "(" {
                    depth += 1;
                } else if sym == ")" {
                    depth = (depth - 1).max(0);
                }
            }
        }
        depth.max(0) as usize
    }

    fn cursor_is_at_locking_clause_keyword_slot_for_context(
        deep_ctx: &intellisense_context::CursorContext,
        exclude_current_identifier_chain: bool,
    ) -> bool {
        if !deep_ctx.phase.is_column_context() {
            return false;
        }
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let end = Self::expected_suggestion_context_end(
            tokens,
            cursor_token_len,
            exclude_current_identifier_chain,
        );
        // The row-locking `FOR` clause is only valid at the statement top level;
        // an open paren means the `FOR` belongs to a function/sub-expression
        // (`SUBSTRING(x FROM a FOR |)`, `MODEL`/`PIVOT` `FOR`).
        if Self::unclosed_paren_count(tokens, end) != 0 {
            return false;
        }
        Self::expected_locking_clause_keyword_candidates(tokens, end).is_some()
    }

    fn collect_expected_keyword_suggestions(
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Vec<String> {
        const TOP_LEVEL_KEYWORDS: &[&str] = &[
            "SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "ALTER", "DROP",
            "BEGIN", "DECLARE", "CALL", "VALUES",
        ];
        const OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "DATABASE",
            "DIRECTORY",
            "TABLESPACE",
            "USER",
            "ROLE",
            "PROFILE",
            "ROLLBACK",
            "JAVA",
            "LIBRARY",
            "CLUSTER",
            "CONTEXT",
            "DIMENSION",
            "OPERATOR",
            "INDEXTYPE",
            "EDITION",
            "PUBLIC",
        ];
        const ALTER_OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "DATABASE",
            "TABLESPACE",
            "USER",
            "ROLE",
            "PROFILE",
            "PUBLIC",
            "SHARED",
            "ROLLBACK",
            "JAVA",
            "LIBRARY",
            "CLUSTER",
            "DIMENSION",
            "OPERATOR",
            "INDEXTYPE",
            "SYSTEM",
            "SESSION",
        ];
        const CREATE_OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "EDITIONING",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "DATABASE",
            "DIRECTORY",
            "TABLESPACE",
            "SHARED",
            "USER",
            "ROLE",
            "PROFILE",
            "ROLLBACK",
            "JAVA",
            "LIBRARY",
            "CLUSTER",
            "CONTEXT",
            "DIMENSION",
            "OPERATOR",
            "INDEXTYPE",
            "EDITION",
            "PUBLIC",
        ];
        const CREATE_OR_REPLACE_OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "EDITIONING",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "DIRECTORY",
            "LIBRARY",
            "USER",
            "JAVA",
            "PUBLIC",
        ];
        const COMMENT_OBJECT_TYPE_KEYWORDS: &[&str] =
            &["COLUMN", "TABLE", "VIEW", "MATERIALIZED", "EDITIONING"];

        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let context_end =
            Self::expected_suggestion_context_end(tokens, cursor_token_len, !prefix.is_empty());
        if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause
        ) && !Self::cursor_is_before_current_query_from_clause(tokens, context_end)
        {
            if let Some(candidates) =
                Self::expected_row_limiting_keyword_candidates(tokens, context_end)
            {
                return Self::filter_expected_candidates(prefix, candidates);
            }
        }

        if let Some(slot) = Self::window_order_by_sort_modifier_slot(tokens, context_end) {
            return Self::filter_expected_candidates(
                prefix,
                window_order_by_sort_modifier_keywords(slot),
            );
        }

        if let Some(candidates) =
            Self::expected_window_frame_keyword_candidates(tokens, context_end)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(candidates) =
            Self::expected_window_spec_start_keyword_candidates(tokens, context_end)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(candidates) =
            Self::expected_window_spec_clause_transition_candidates(tokens, context_end)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(slot) = Self::keep_dense_rank_slot(tokens, context_end) {
            return Self::filter_expected_candidates(prefix, keep_dense_rank_keywords(slot));
        }

        if let Some(slot) = Self::within_group_slot(tokens, context_end) {
            return Self::filter_expected_candidates(prefix, within_group_keywords(slot));
        }

        if let Some(slot) = Self::analytic_null_treatment_slot(tokens, context_end) {
            return Self::filter_expected_candidates(
                prefix,
                analytic_null_treatment_keywords(slot),
            );
        }

        if let Some(candidates) = Self::type_attribute_candidates(tokens, context_end) {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(candidates) =
            Self::expected_sounds_like_keyword_candidates(tokens, context_end, db_type)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(candidates) =
            Self::expected_json_on_target_keyword_candidates(tokens, context_end, db_type)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        // GRANT/REVOKE privilege keywords. Emission only — these merge with the
        // identifier base (a role name is also grantable here), so the privilege
        // slot is deliberately left out of the suppression chokepoint.
        if let Some(candidates) =
            Self::expected_grant_privilege_keyword_candidates(tokens, context_end)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(position) = Self::data_type_position(tokens, context_end) {
            return Self::filter_expected_candidates(
                prefix,
                data_type_keywords_for(db_type, position),
            );
        }

        match Self::extract_field_position(tokens, context_end) {
            Some(ExtractArgPosition::Field) => {
                return Self::filter_expected_candidates(
                    prefix,
                    extract_field_keywords_for(db_type),
                );
            }
            Some(ExtractArgPosition::AwaitingFrom) => {
                return Self::filter_expected_candidates(prefix, &["FROM"]);
            }
            None => {}
        }

        if let Some(slot) = Self::interval_unit_position(tokens, context_end) {
            return Self::filter_expected_candidates(
                prefix,
                interval_unit_keywords_for(db_type, slot),
            );
        }

        if let Some(slot) =
            Self::order_by_sort_modifier_slot(tokens, context_end, deep_ctx.phase)
        {
            return Self::filter_expected_candidates(prefix, order_by_sort_modifier_keywords(slot));
        }

        if let Some(candidates) =
            Self::expected_dml_target_keyword_candidates(tokens, context_end, deep_ctx.phase)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        if let Some(candidates) =
            Self::expected_join_target_keyword_candidates(tokens, context_end, deep_ctx.phase)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }

        let words = Self::previous_meaningful_words_upper(tokens, context_end, 6);
        if let Some(candidates) = Self::expected_database_link_keyword_candidates(&words) {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if let Some(candidates) = Self::expected_java_keyword_candidates(&words) {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if let Some(candidates) = Self::expected_rollback_segment_keyword_candidates(&words) {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if let Some(candidates) = Self::merge_then_action_keywords(tokens, context_end) {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if let Some(candidates) = Self::merge_when_action_keywords(tokens, context_end) {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if let Some(candidates) = Self::expected_set_operator_keyword_candidates(tokens, context_end)
        {
            return Self::filter_expected_candidates(prefix, candidates);
        }
        if Self::cursor_is_at_locking_clause_keyword_slot_for_context(deep_ctx, !prefix.is_empty()) {
            if let Some(candidates) =
                Self::expected_locking_clause_keyword_candidates(tokens, context_end)
            {
                return Self::filter_expected_candidates(prefix, candidates);
            }
        }

        // A join continuation (`LEFT|RIGHT|FULL|INNER|CROSS|NATURAL JOIN`) is only
        // grammatical in a table position. Gating on the phase keeps a column or
        // function named `left`/`right`/… (`SELECT left |`, `WHERE right |`) from
        // wrongly offering `JOIN`, while `FROM a LEFT |` stays in `FromClause`.
        let in_table_context = deep_ctx.phase.is_table_context();
        // A dual-use clause keyword (`ORDER`/`GROUP`/`CONNECT`/`START`) used as a
        // qualified member (`t.order `, `t.start `) is a column, so its `BY`/`WITH`
        // continuation must not be offered.
        let trigger_is_qualified_member =
            Self::trigger_word_is_qualified_member(tokens, context_end);

        let candidates: &[&str] = match words.as_slice() {
            // `previous_meaningful_words_upper` stops at the first non-word token
            // (a string literal, and any other non-identifier value token), so an
            // empty `words` is ambiguous: it is a genuine statement start *only*
            // when nothing at all precedes the cursor. When a value operand sits
            // immediately before it (`WHERE c = 'x' |`, `IN ('a', |`,
            // `VALUES (1, |`) the cursor is mid-expression, never a place to begin
            // a new statement — so the top-level statement keywords would be pure
            // noise. Gate the statement-start list on there being no preceding
            // token at all; the operator/clause continuations valid after the
            // operand still arrive through the expression-keyword allowlist.
            [] if Self::meaningful_tokens_before(tokens, context_end).is_empty() => {
                TOP_LEVEL_KEYWORDS
            }
            [] => &[],
            _ if Self::is_create_synonym_name_written_context(&words) => &["FOR"],
            [.., last]
                if !trigger_is_qualified_member
                    && (*last == "ORDER" || *last == "GROUP" || *last == "CONNECT") =>
            {
                &["BY"]
            }
            // `PARTITION BY` inside an analytic `OVER (...)` / `WINDOW name AS
            // (...)` spec. Gated on the window context so the non-`BY` uses of
            // `PARTITION` keep their identifier completion: a partition-extended
            // table reference (`FROM t PARTITION (p)`) and the DDL partition-
            // maintenance ops (`ALTER TABLE t DROP PARTITION p`) both expect a
            // partition name, not `BY`.
            [.., last]
                if !trigger_is_qualified_member
                    && *last == "PARTITION"
                    && Self::cursor_is_inside_window_spec(tokens, context_end) =>
            {
                &["BY"]
            }
            [.., prev, last] if *prev == "ORDER" && *last == "SIBLINGS" => &["BY"],
            [.., last] if !trigger_is_qualified_member && *last == "START" => &["WITH"],
            // Foreign-key referential actions: `ON DELETE |` / `ON UPDATE |`. The
            // `ON UPDATE` slot additionally admits the MySQL column-default
            // `CURRENT_TIMESTAMP`. Anchored on `ON` so a DML `DELETE`/`UPDATE`
            // statement keyword keeps its own continuation below.
            [.., prev, last] if *prev == "ON" && *last == "DELETE" => {
                &["CASCADE", "SET NULL", "SET DEFAULT", "NO ACTION", "RESTRICT"]
            }
            [.., prev, last] if *prev == "ON" && *last == "UPDATE" => &[
                "CASCADE",
                "SET NULL",
                "SET DEFAULT",
                "NO ACTION",
                "RESTRICT",
                "CURRENT_TIMESTAMP",
            ],
            // `<expr> IS NOT |` -> NULL; `<expr> IS |` -> NOT / NULL. Only keywords
            // are grammatical after `IS`, so the matching suppression predicate
            // (`cursor_is_at_is_null_test_keyword_position_for_context`) clears the
            // identifier list there.
            [.., prev, last] if *prev == "IS" && *last == "NOT" => &["NULL"],
            [.., last] if !trigger_is_qualified_member && *last == "IS" => &["NOT", "NULL"],
            [.., last] if *last == "DELETE" => &["FROM"],
            [.., last] if *last == "INSERT" || *last == "MERGE" => &["INTO"],
            // `LEFT`/`RIGHT`/`FULL` may be followed by the optional `OUTER`
            // before `JOIN`, so both continuations are offered; `INNER`/`CROSS`/
            // `NATURAL` take only `JOIN`.
            [.., last]
                if in_table_context && matches!(last.as_str(), "LEFT" | "RIGHT" | "FULL") =>
            {
                &["OUTER", "JOIN"]
            }
            [.., last]
                if in_table_context
                    && matches!(last.as_str(), "INNER" | "CROSS" | "NATURAL") =>
            {
                &["JOIN"]
            }
            [.., prev, last]
                if in_table_context
                    && *last == "OUTER"
                    && matches!(prev.as_str(), "LEFT" | "RIGHT" | "FULL") =>
            {
                &["JOIN"]
            }
            [.., last] if *last == "CREATE" => CREATE_OBJECT_TYPE_KEYWORDS,
            [.., last] if *last == "DROP" => OBJECT_TYPE_KEYWORDS,
            [.., last] if *last == "ALTER" => ALTER_OBJECT_TYPE_KEYWORDS,
            [.., prev, last] if *prev == "CREATE" && matches!(last.as_str(), "UNIQUE" | "BITMAP") => {
                &["INDEX"]
            }
            [.., prev, last] if *prev == "CREATE" && *last == "GLOBAL" => &["TEMPORARY"],
            [.., a, b, c] if *a == "CREATE" && *b == "GLOBAL" && *c == "TEMPORARY" => {
                &["TABLE"]
            }
            [.., prev, last]
                if matches!(prev.as_str(), "CREATE" | "DROP" | "ALTER" | "ON")
                    && *last == "MATERIALIZED" =>
            {
                &["VIEW"]
            }
            [.., a, b, c, d]
                if *a == "CREATE" && *b == "OR" && *c == "REPLACE" && *d == "MATERIALIZED" =>
            {
                &["VIEW"]
            }
            [.., a, b, c]
                if matches!(a.as_str(), "ALTER" | "CREATE" | "DROP")
                    && *b == "MATERIALIZED"
                    && *c == "VIEW" =>
            {
                &["LOG"]
            }
            [.., a, b, c, d]
                if matches!(a.as_str(), "ALTER" | "CREATE" | "DROP")
                    && *b == "MATERIALIZED"
                    && *c == "VIEW"
                    && *d == "LOG" =>
            {
                &["ON"]
            }
            [.., prev, last]
                if matches!(prev.as_str(), "CREATE" | "ON") && *last == "EDITIONING" =>
            {
                &["VIEW"]
            }
            [.., a, b, c, d]
                if *a == "CREATE" && *b == "OR" && *c == "REPLACE" && *d == "EDITIONING" =>
            {
                &["VIEW"]
            }
            [.., prev, last]
                if matches!(prev.as_str(), "CREATE" | "DROP")
                    && matches!(last.as_str(), "PACKAGE" | "TYPE") =>
            {
                &["BODY"]
            }
            [.., a, b, c, d]
                if *a == "CREATE"
                    && *b == "OR"
                    && *c == "REPLACE"
                    && matches!(d.as_str(), "PACKAGE" | "TYPE") =>
            {
                &["BODY"]
            }
            [.., prev, last] if *prev == "CREATE" && *last == "PUBLIC" => {
                &["SYNONYM", "DATABASE", "ROLLBACK"]
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "PUBLIC" =>
            {
                &["SYNONYM", "DATABASE"]
            }
            [.., a, b, c, d]
                if *a == "CREATE" && *b == "OR" && *c == "REPLACE" && *d == "PUBLIC" =>
            {
                &["SYNONYM"]
            }
            [.., prev, last] if *prev == "COMMENT" && *last == "ON" => {
                COMMENT_OBJECT_TYPE_KEYWORDS
            }
            [.., a, b, c] if *a == "CREATE" && *b == "OR" && *c == "REPLACE" => {
                CREATE_OR_REPLACE_OBJECT_TYPE_KEYWORDS
            }
            [.., last] if *last == "TRUNCATE" || *last == "LOCK" || *last == "FLASHBACK" => {
                &["TABLE"]
            }
            [.., last]
                if matches!(
                    last.as_str(),
                    "ANALYZE" | "OPTIMIZE" | "CHECK" | "REPAIR"
                ) =>
            {
                &["TABLE"]
            }
            [.., prev, last] if *prev == "ALTER" && *last == "SESSION" => &["SET"],
            [.., a, b, c] if *a == "ALTER" && *b == "SESSION" && *c == "SET" => {
                &["CURRENT_SCHEMA"]
            }
            [.., last] if *last == "COMMENT" => &["ON"],
            [.., last] if *last == "EXECUTE" => &["IMMEDIATE"],
            [.., prev, last] if *prev == "CREATE" && *last == "OR" => &["REPLACE"],
            _ => {
                if deep_ctx.cursor_token_len == 0 {
                    TOP_LEVEL_KEYWORDS
                } else {
                    &[]
                }
            }
        };

        Self::filter_expected_candidates(prefix, candidates)
    }

    fn is_row_count_tail_word(word: &str, is_bind: bool) -> bool {
        let trimmed = word.trim();
        if trimmed.is_empty() || Self::is_fetch_row_limit_direction(trimmed) {
            return false;
        }
        if is_bind {
            return true;
        }
        !Self::is_row_limit_unit(trimmed) && !matches!(trimmed, "PERCENT" | "ONLY" | "WITH" | "TIES")
    }

    fn is_fetch_row_limit_direction(word: &str) -> bool {
        matches!(word, "FIRST" | "NEXT")
    }

    fn is_row_limit_unit(word: &str) -> bool {
        matches!(word, "ROW" | "ROWS")
    }

    fn fetch_row_limit_prefix_before_unit(
        words: &[(String, bool)],
        unit_from_end: usize,
    ) -> bool {
        let len = words.len();
        let word = |from_end: usize| {
            len.checked_sub(from_end)
                .and_then(|idx| words.get(idx))
                .map(|(value, _)| value.as_str())
        };
        let is_count = |from_end: usize| {
            len.checked_sub(from_end)
                .and_then(|idx| words.get(idx))
                .is_some_and(|(value, is_bind)| Self::is_row_count_tail_word(value, *is_bind))
        };

        (word(unit_from_end + 1).is_some_and(Self::is_fetch_row_limit_direction)
            && word(unit_from_end + 2) == Some("FETCH"))
            || (is_count(unit_from_end + 1)
                && word(unit_from_end + 2).is_some_and(Self::is_fetch_row_limit_direction)
                && word(unit_from_end + 3) == Some("FETCH"))
            || (word(unit_from_end + 1) == Some("PERCENT")
                && is_count(unit_from_end + 2)
                && word(unit_from_end + 3).is_some_and(Self::is_fetch_row_limit_direction)
                && word(unit_from_end + 4) == Some("FETCH"))
    }

    /// True when a row-limiting clause is syntactically complete at the cursor
    /// and therefore admits no bare identifier unless another clause delimiter is
    /// written first. This complements `expected_row_limiting_keyword_candidates`,
    /// which covers the intermediate slots that still have a fixed keyword
    /// continuation (`ROWS |` -> `ONLY`/`WITH`, `LIMIT n |` -> `OFFSET`).
    fn cursor_is_after_complete_row_limiting_tail(
        tokens: &[SqlToken],
        end: usize,
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        let words = Self::previous_meaningful_words_with_bind_markers_upper(tokens, end, 6);
        let len = words.len();
        let word = |from_end: usize| {
            len.checked_sub(from_end)
                .and_then(|idx| words.get(idx))
                .map(|(value, _)| value.as_str())
        };
        let is_count = |from_end: usize| {
            len.checked_sub(from_end)
                .and_then(|idx| words.get(idx))
                .is_some_and(|(value, is_bind)| Self::is_row_count_tail_word(value, *is_bind))
        };

        // ANSI/Oracle: `FETCH FIRST|NEXT [n [PERCENT]] ROWS ONLY`.
        if word(1) == Some("ONLY")
            && word(2).is_some_and(Self::is_row_limit_unit)
            && Self::fetch_row_limit_prefix_before_unit(&words, 2)
        {
            return true;
        }
        // ANSI/Oracle: `FETCH FIRST|NEXT [n [PERCENT]] ROWS WITH TIES`.
        if word(1) == Some("TIES")
            && word(2) == Some("WITH")
            && word(3).is_some_and(Self::is_row_limit_unit)
            && Self::fetch_row_limit_prefix_before_unit(&words, 3)
        {
            return true;
        }
        if !matches!(phase, intellisense_context::SqlPhase::OrderByClause) {
            return false;
        }
        // MySQL/MariaDB: `LIMIT count OFFSET offset`.
        if len >= 4
            && word(4) == Some("LIMIT")
            && is_count(3)
            && word(2) == Some("OFFSET")
            && is_count(1)
        {
            return true;
        }
        // MySQL/MariaDB: `LIMIT offset, count` (commas are skipped by the word
        // collector, so this deliberately also suppresses the invalid
        // `LIMIT offset count` lookalike instead of leaking identifiers).
        if len >= 3 && word(3) == Some("LIMIT") && is_count(2) && is_count(1) {
            return true;
        }
        false
    }

    fn expected_object_suggestion_kind(
        prefix: &str,
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Option<ExpectedObjectSuggestionKind> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let words = Self::previous_meaningful_words_upper(
            tokens,
            Self::expected_suggestion_context_end(
                tokens,
                cursor_token_len,
                !prefix.is_empty() || qualifier.is_some(),
            ),
            16,
        );

        if let Some(kind) = Self::expected_grant_revoke_object_suggestion_kind(&words) {
            return Some(kind);
        }
        if let Some(kind) = Self::expected_grant_revoke_grantee_kind(&words) {
            return Some(kind);
        }
        if Self::is_create_synonym_target_context(&words) {
            return Some(ExpectedObjectSuggestionKind::Any);
        }
        if Self::is_create_on_table_target_context(&words) {
            return Some(ExpectedObjectSuggestionKind::Table);
        }

        match words.as_slice() {
            [.., last] if matches!(last.as_str(), "CALL" | "EXEC" | "EXECUTE") => {
                Some(ExpectedObjectSuggestionKind::Routine)
            }
            [.., last] if matches!(last.as_str(), "DESC" | "DESCRIBE") => {
                Some(ExpectedObjectSuggestionKind::Any)
            }
            [.., last] if *last == "REFERENCES" => Some(ExpectedObjectSuggestionKind::Table),
            [.., prev, last]
                if matches!(
                    prev.as_str(),
                    "ALTER"
                        | "DROP"
                        | "TRUNCATE"
                        | "FLASHBACK"
                        | "LOCK"
                        | "ANALYZE"
                        | "OPTIMIZE"
                        | "CHECK"
                        | "REPAIR"
                ) && *last == "TABLE" =>
            {
                Some(ExpectedObjectSuggestionKind::Table)
            }
            [.., a, b, c] if *a == "COMMENT" && *b == "ON" && *c == "TABLE" => {
                Some(ExpectedObjectSuggestionKind::Table)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "VIEW" => {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c] if *a == "COMMENT" && *b == "ON" && *c == "VIEW" => {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c, d]
                if *a == "COMMENT" && *b == "ON" && *c == "EDITIONING" && *d == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c]
                if matches!(a.as_str(), "ALTER" | "DROP")
                    && *b == "MATERIALIZED"
                    && *c == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::MaterializedView)
            }
            [.., a, b, c, d]
                if *a == "COMMENT" && *b == "ON" && *c == "MATERIALIZED" && *d == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::MaterializedView)
            }
            [.., a, b, c, d, e]
                if matches!(a.as_str(), "ALTER" | "CREATE" | "DROP")
                    && *b == "MATERIALIZED"
                    && *c == "VIEW"
                    && *d == "LOG"
                    && *e == "ON" =>
            {
                Some(ExpectedObjectSuggestionKind::Table)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "TYPE" => {
                Some(ExpectedObjectSuggestionKind::Type)
            }
            [.., a, b, c] if *a == "DROP" && *b == "TYPE" && *c == "BODY" => {
                Some(ExpectedObjectSuggestionKind::Type)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "TRIGGER" =>
            {
                Some(ExpectedObjectSuggestionKind::Trigger)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "EVENT" => {
                Some(ExpectedObjectSuggestionKind::Event)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "INDEX" => {
                Some(ExpectedObjectSuggestionKind::Index)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "PROCEDURE" =>
            {
                Some(ExpectedObjectSuggestionKind::Procedure)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "FUNCTION" =>
            {
                Some(ExpectedObjectSuggestionKind::Function)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "PACKAGE" => {
                Some(ExpectedObjectSuggestionKind::Package)
            }
            [.., a, b, c] if *a == "DROP" && *b == "PACKAGE" && *c == "BODY" => {
                Some(ExpectedObjectSuggestionKind::Package)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "SEQUENCE" =>
            {
                Some(ExpectedObjectSuggestionKind::Sequence)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "SYNONYM" => {
                Some(ExpectedObjectSuggestionKind::Synonym)
            }
            [.., a, b, c]
                if matches!(a.as_str(), "ALTER" | "DROP") && *b == "PUBLIC" && *c == "SYNONYM" =>
            {
                Some(ExpectedObjectSuggestionKind::PublicSynonym)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "DATABASE" =>
            {
                None
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "LINK" =>
            {
                Some(ExpectedObjectSuggestionKind::DatabaseLink)
            }
            [.., a, b, c]
                if matches!(a.as_str(), "ALTER" | "DROP") && *b == "DATABASE" && *c == "LINK" =>
            {
                Some(ExpectedObjectSuggestionKind::DatabaseLink)
            }
            [.., a, b, c, d]
                if matches!(a.as_str(), "ALTER" | "DROP")
                    && *b == "PUBLIC"
                    && *c == "DATABASE"
                    && *d == "LINK" =>
            {
                Some(ExpectedObjectSuggestionKind::DatabaseLink)
            }
            [.., prev, last] if *prev == "DROP" && *last == "DIRECTORY" => {
                Some(ExpectedObjectSuggestionKind::Directory)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "LIBRARY" =>
            {
                Some(ExpectedObjectSuggestionKind::Library)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "CLUSTER" =>
            {
                Some(ExpectedObjectSuggestionKind::Cluster)
            }
            [.., prev, last] if *prev == "DROP" && *last == "CONTEXT" => {
                Some(ExpectedObjectSuggestionKind::Context)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "DIMENSION" =>
            {
                Some(ExpectedObjectSuggestionKind::Dimension)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "OPERATOR" =>
            {
                Some(ExpectedObjectSuggestionKind::Operator)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "INDEXTYPE" =>
            {
                Some(ExpectedObjectSuggestionKind::Indextype)
            }
            [.., prev, last] if *prev == "DROP" && *last == "EDITION" => {
                Some(ExpectedObjectSuggestionKind::Edition)
            }
            [.., prev, java, object_type]
                if matches!(prev.as_str(), "ALTER" | "DROP")
                    && *java == "JAVA"
                    && *object_type == "SOURCE" =>
            {
                Some(ExpectedObjectSuggestionKind::JavaSource)
            }
            [.., prev, java, object_type]
                if matches!(prev.as_str(), "ALTER" | "DROP")
                    && *java == "JAVA"
                    && *object_type == "CLASS" =>
            {
                Some(ExpectedObjectSuggestionKind::JavaClass)
            }
            [.., prev, java, object_type]
                if *prev == "DROP" && *java == "JAVA" && *object_type == "RESOURCE" =>
            {
                Some(ExpectedObjectSuggestionKind::JavaResource)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "USER" => {
                Some(ExpectedObjectSuggestionKind::User)
            }
            [.., a, b, c, d]
                if *a == "ALTER" && *b == "SESSION" && *c == "SET" && *d == "CURRENT_SCHEMA" =>
            {
                Some(ExpectedObjectSuggestionKind::User)
            }
            _ => None,
        }
    }

    /// The grantee slot of `GRANT … TO <grantee>` / `REVOKE … FROM <grantee>`
    /// (including a role grant, `GRANT role TO <grantee>`) names a user or role,
    /// never a schema object. Resolves the slot right after the `TO`/`FROM`
    /// separator to the `User` kind so the whole-catalog dump is suppressed
    /// there. A multi-grantee continuation after a comma keeps today's behavior
    /// (the comma is not a meaningful word, so the separator is no longer last).
    fn expected_grant_revoke_grantee_kind(
        words: &[String],
    ) -> Option<ExpectedObjectSuggestionKind> {
        let separator = words.last()?;
        if !matches!(separator.as_str(), "TO" | "FROM") {
            return None;
        }
        let verb = words.iter().rev().find_map(|word| match word.as_str() {
            "GRANT" | "REVOKE" => Some(word.as_str()),
            _ => None,
        })?;
        let expected_separator = if verb == "GRANT" { "TO" } else { "FROM" };
        (separator == expected_separator).then_some(ExpectedObjectSuggestionKind::User)
    }

    fn expected_grant_revoke_object_suggestion_kind(
        words: &[String],
    ) -> Option<ExpectedObjectSuggestionKind> {
        if words.last().is_none_or(|word| word != "ON") {
            return None;
        }
        let verb_idx = words
            .iter()
            .rposition(|word| matches!(word.as_str(), "GRANT" | "REVOKE" | "AUDIT" | "NOAUDIT"))?;
        let privilege_words = words.get(verb_idx + 1..words.len().saturating_sub(1))?;
        if privilege_words.is_empty() {
            return None;
        }
        // `AUDIT`/`NOAUDIT` object options apply to objects of any type
        // (tables, views, sequences, procedures, …), and their option list is
        // not restricted to the GRANT privilege sets, so any non-empty option
        // list resolves the `ON` slot to an object of any kind.
        if matches!(words[verb_idx].as_str(), "AUDIT" | "NOAUDIT") {
            return Some(ExpectedObjectSuggestionKind::Any);
        }
        if privilege_words
            .iter()
            .all(|word| Self::is_executable_object_privilege(word))
        {
            return Some(ExpectedObjectSuggestionKind::Executable);
        }
        if privilege_words
            .iter()
            .all(|word| Self::is_relation_or_sequence_object_privilege(word))
        {
            return Some(ExpectedObjectSuggestionKind::RelationOrSequence);
        }

        None
    }

    fn is_executable_object_privilege(word: &str) -> bool {
        matches!(word, "EXECUTE" | "DEBUG")
    }

    fn is_relation_or_sequence_object_privilege(word: &str) -> bool {
        matches!(
            word,
            "SELECT"
                | "READ"
                | "INSERT"
                | "UPDATE"
                | "DELETE"
                | "REFERENCES"
                | "INDEX"
                | "ALTER"
                | "FLASHBACK"
                | "QUERY"
                | "REWRITE"
        )
    }

    fn expected_suggestion_context_end(
        tokens: &[SqlToken],
        cursor_token_len: usize,
        exclude_current_identifier_chain: bool,
    ) -> usize {
        if !exclude_current_identifier_chain {
            return cursor_token_len.min(tokens.len());
        }

        Self::current_qualified_identifier_chain_start(tokens, cursor_token_len)
            .unwrap_or(cursor_token_len)
            .min(tokens.len())
    }

    fn collect_expected_object_suggestions_for_kind(
        data: &mut IntellisenseData,
        prefix: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> Vec<String> {
        let suggestions = match kind {
            ExpectedObjectSuggestionKind::Any => data.get_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Routine => data.get_routine_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Executable => data.get_executable_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::RelationOrSequence => {
                data.get_relation_or_sequence_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Table => data.get_table_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::View => data.get_view_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::MaterializedView => {
                data.get_materialized_view_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Type => data.get_type_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Trigger => data.get_trigger_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Event => data.get_event_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Index => data.get_index_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Procedure => {
                data.get_procedure_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Function => data.get_function_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Package => data.get_package_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Sequence => data.get_sequence_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Synonym => data.get_synonym_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::PublicSynonym => {
                data.get_public_synonym_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::DatabaseLink => {
                data.get_database_link_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Directory => data.get_directory_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Library => data.get_library_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Cluster => data.get_cluster_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Context => data.get_context_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Dimension => data.get_dimension_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Operator => data.get_operator_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Indextype => data.get_indextype_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Edition => data.get_edition_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::JavaSource => {
                data.get_java_source_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::JavaClass => data.get_java_class_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::JavaResource => {
                data.get_java_resource_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::User => data.get_user_suggestions(prefix),
        };

        if prefix.is_empty() || matches!(kind, ExpectedObjectSuggestionKind::User) {
            return suggestions;
        }

        Self::merge_suggestions_with_context_aliases(
            suggestions,
            data.get_user_suggestions(prefix),
            false,
        )
    }

    fn matches_string_list_case_insensitive(values: &[String], candidate: &str) -> bool {
        values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(candidate))
    }

    fn suggestion_matches_expected_object_kind(
        data: &IntellisenseData,
        candidate: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> bool {
        match kind {
            ExpectedObjectSuggestionKind::Any => true,
            ExpectedObjectSuggestionKind::Routine => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
                    || Self::matches_string_list_case_insensitive(&data.functions, candidate)
                    || Self::matches_string_list_case_insensitive(&data.packages, candidate)
            }
            ExpectedObjectSuggestionKind::Executable => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
                    || Self::matches_string_list_case_insensitive(&data.functions, candidate)
                    || Self::matches_string_list_case_insensitive(&data.packages, candidate)
                    || Self::matches_string_list_case_insensitive(&data.types, candidate)
            }
            ExpectedObjectSuggestionKind::RelationOrSequence => {
                Self::matches_string_list_case_insensitive(&data.tables, candidate)
                    || Self::matches_string_list_case_insensitive(&data.views, candidate)
                    || Self::matches_string_list_case_insensitive(&data.materialized_views, candidate)
                    || Self::matches_string_list_case_insensitive(&data.sequences, candidate)
                    || Self::matches_string_list_case_insensitive(&data.synonyms, candidate)
                    || Self::matches_string_list_case_insensitive(&data.public_synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::Table => {
                Self::matches_string_list_case_insensitive(&data.tables, candidate)
            }
            ExpectedObjectSuggestionKind::View => {
                Self::matches_string_list_case_insensitive(&data.views, candidate)
            }
            ExpectedObjectSuggestionKind::MaterializedView => {
                Self::matches_string_list_case_insensitive(&data.materialized_views, candidate)
            }
            ExpectedObjectSuggestionKind::Type => {
                Self::matches_string_list_case_insensitive(&data.types, candidate)
            }
            ExpectedObjectSuggestionKind::Trigger => {
                Self::matches_string_list_case_insensitive(&data.triggers, candidate)
            }
            ExpectedObjectSuggestionKind::Event => {
                Self::matches_string_list_case_insensitive(&data.events, candidate)
            }
            ExpectedObjectSuggestionKind::Index => {
                Self::matches_string_list_case_insensitive(&data.indexes, candidate)
            }
            ExpectedObjectSuggestionKind::Procedure => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
            }
            ExpectedObjectSuggestionKind::Function => {
                Self::matches_string_list_case_insensitive(&data.functions, candidate)
            }
            ExpectedObjectSuggestionKind::Package => {
                Self::matches_string_list_case_insensitive(&data.packages, candidate)
            }
            ExpectedObjectSuggestionKind::Sequence => {
                Self::matches_string_list_case_insensitive(&data.sequences, candidate)
            }
            ExpectedObjectSuggestionKind::Synonym => {
                Self::matches_string_list_case_insensitive(&data.synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::PublicSynonym => {
                Self::matches_string_list_case_insensitive(&data.public_synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::DatabaseLink => {
                Self::matches_string_list_case_insensitive(&data.database_links, candidate)
            }
            ExpectedObjectSuggestionKind::Directory => {
                Self::matches_string_list_case_insensitive(&data.directories, candidate)
            }
            ExpectedObjectSuggestionKind::Library => {
                Self::matches_string_list_case_insensitive(&data.libraries, candidate)
            }
            ExpectedObjectSuggestionKind::Cluster => {
                Self::matches_string_list_case_insensitive(&data.clusters, candidate)
            }
            ExpectedObjectSuggestionKind::Context => {
                Self::matches_string_list_case_insensitive(&data.contexts, candidate)
            }
            ExpectedObjectSuggestionKind::Dimension => {
                Self::matches_string_list_case_insensitive(&data.dimensions, candidate)
            }
            ExpectedObjectSuggestionKind::Operator => {
                Self::matches_string_list_case_insensitive(&data.operators, candidate)
            }
            ExpectedObjectSuggestionKind::Indextype => {
                Self::matches_string_list_case_insensitive(&data.indextypes, candidate)
            }
            ExpectedObjectSuggestionKind::Edition => {
                Self::matches_string_list_case_insensitive(&data.editions, candidate)
            }
            ExpectedObjectSuggestionKind::JavaSource => {
                Self::matches_string_list_case_insensitive(&data.java_sources, candidate)
            }
            ExpectedObjectSuggestionKind::JavaClass => {
                Self::matches_string_list_case_insensitive(&data.java_classes, candidate)
            }
            ExpectedObjectSuggestionKind::JavaResource => {
                Self::matches_string_list_case_insensitive(&data.java_resources, candidate)
            }
            ExpectedObjectSuggestionKind::User => {
                Self::matches_string_list_case_insensitive(&data.users, candidate)
            }
        }
    }

    fn expected_qualifier_member_kinds(
        kind: ExpectedObjectSuggestionKind,
    ) -> Option<&'static [QualifiedMemberKind]> {
        match kind {
            ExpectedObjectSuggestionKind::Any => None,
            ExpectedObjectSuggestionKind::Routine => Some(&[
                QualifiedMemberKind::Procedure,
                QualifiedMemberKind::Function,
                QualifiedMemberKind::Package,
            ]),
            ExpectedObjectSuggestionKind::Executable => Some(&[
                QualifiedMemberKind::Procedure,
                QualifiedMemberKind::Function,
                QualifiedMemberKind::Package,
                QualifiedMemberKind::Type,
            ]),
            ExpectedObjectSuggestionKind::RelationOrSequence => Some(&[
                QualifiedMemberKind::Table,
                QualifiedMemberKind::View,
                QualifiedMemberKind::MaterializedView,
                QualifiedMemberKind::Sequence,
                QualifiedMemberKind::Synonym,
                QualifiedMemberKind::PublicSynonym,
            ]),
            ExpectedObjectSuggestionKind::Table => Some(&[QualifiedMemberKind::Table]),
            ExpectedObjectSuggestionKind::View => Some(&[QualifiedMemberKind::View]),
            ExpectedObjectSuggestionKind::MaterializedView => {
                Some(&[QualifiedMemberKind::MaterializedView])
            }
            ExpectedObjectSuggestionKind::Type => Some(&[QualifiedMemberKind::Type]),
            ExpectedObjectSuggestionKind::Trigger => Some(&[QualifiedMemberKind::Trigger]),
            ExpectedObjectSuggestionKind::Event => Some(&[QualifiedMemberKind::Event]),
            ExpectedObjectSuggestionKind::Index => Some(&[QualifiedMemberKind::Index]),
            ExpectedObjectSuggestionKind::Procedure => Some(&[QualifiedMemberKind::Procedure]),
            ExpectedObjectSuggestionKind::Function => Some(&[QualifiedMemberKind::Function]),
            ExpectedObjectSuggestionKind::Package => Some(&[QualifiedMemberKind::Package]),
            ExpectedObjectSuggestionKind::Sequence => Some(&[QualifiedMemberKind::Sequence]),
            ExpectedObjectSuggestionKind::Synonym => Some(&[QualifiedMemberKind::Synonym]),
            ExpectedObjectSuggestionKind::PublicSynonym => {
                Some(&[QualifiedMemberKind::PublicSynonym])
            }
            ExpectedObjectSuggestionKind::DatabaseLink => Some(&[QualifiedMemberKind::DatabaseLink]),
            ExpectedObjectSuggestionKind::Directory => Some(&[QualifiedMemberKind::Directory]),
            ExpectedObjectSuggestionKind::Library => Some(&[QualifiedMemberKind::Library]),
            ExpectedObjectSuggestionKind::Cluster => Some(&[QualifiedMemberKind::Cluster]),
            ExpectedObjectSuggestionKind::Context => Some(&[QualifiedMemberKind::Context]),
            ExpectedObjectSuggestionKind::Dimension => Some(&[QualifiedMemberKind::Dimension]),
            ExpectedObjectSuggestionKind::Operator => Some(&[QualifiedMemberKind::Operator]),
            ExpectedObjectSuggestionKind::Indextype => Some(&[QualifiedMemberKind::Indextype]),
            ExpectedObjectSuggestionKind::Edition => Some(&[QualifiedMemberKind::Edition]),
            ExpectedObjectSuggestionKind::JavaSource => Some(&[QualifiedMemberKind::JavaSource]),
            ExpectedObjectSuggestionKind::JavaClass => Some(&[QualifiedMemberKind::JavaClass]),
            ExpectedObjectSuggestionKind::JavaResource => Some(&[QualifiedMemberKind::JavaResource]),
            ExpectedObjectSuggestionKind::User => Some(&[QualifiedMemberKind::User]),
        }
    }

    fn suggestion_matches_expected_object_kind_for_qualifier(
        data: &IntellisenseData,
        qualifier: &str,
        candidate: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> bool {
        if let Some(expected_kinds) = Self::expected_qualifier_member_kinds(kind) {
            if let Some(matches) =
                data.qualifier_member_matches_kinds(qualifier, candidate, expected_kinds)
            {
                return matches;
            }
        }

        Self::suggestion_matches_expected_object_kind(data, candidate, kind)
    }

    fn expected_member_suggestions_for_qualifier(
        data: &mut IntellisenseData,
        qualifier: &str,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let suggestions = data.get_member_suggestions(qualifier, prefix, false);
        let Some(kind) = Self::expected_object_suggestion_kind(prefix, Some(qualifier), deep_ctx)
        else {
            return suggestions;
        };
        if matches!(kind, ExpectedObjectSuggestionKind::Any) {
            return suggestions;
        }

        let mut filtered = Vec::new();
        let mut saw_kind_metadata = false;
        let mut seen = HashSet::new();
        for suggestion in &suggestions {
            if let Some(expected_kinds) = Self::expected_qualifier_member_kinds(kind) {
                if data
                    .qualifier_member_matches_kinds(qualifier, suggestion, expected_kinds)
                    .is_some()
                {
                    saw_kind_metadata = true;
                }
            }
            if !Self::suggestion_matches_expected_object_kind_for_qualifier(
                data,
                qualifier,
                suggestion,
                kind,
            ) {
                continue;
            }
            if seen.insert(Self::completion_identifier_lookup_upper(suggestion)) {
                filtered.push(suggestion.clone());
            }
            if filtered.len() >= MAX_MERGED_SUGGESTIONS {
                break;
            }
        }
        if filtered.is_empty() && !saw_kind_metadata {
            suggestions
        } else {
            filtered
        }
    }

    fn expected_relation_member_suggestions_for_qualifier(
        data: &mut IntellisenseData,
        qualifier: &str,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let suggestions = data.get_member_suggestions(qualifier, prefix, true);
        let Some(kind) = Self::expected_object_suggestion_kind(prefix, Some(qualifier), deep_ctx)
        else {
            return suggestions;
        };
        if matches!(kind, ExpectedObjectSuggestionKind::Any) {
            let all_suggestions = data.get_member_suggestions(qualifier, prefix, false);
            return if all_suggestions.is_empty() {
                suggestions
            } else {
                all_suggestions
            };
        }
        let Some(expected_kinds) = Self::expected_qualifier_member_kinds(kind) else {
            return suggestions;
        };

        let mut filtered = Vec::new();
        let mut saw_kind_metadata = false;
        let mut seen = HashSet::new();
        for suggestion in &suggestions {
            let Some(matches) =
                data.qualifier_member_matches_kinds(qualifier, suggestion, expected_kinds)
            else {
                continue;
            };
            saw_kind_metadata = true;
            if matches && seen.insert(Self::completion_identifier_lookup_upper(suggestion)) {
                filtered.push(suggestion.clone());
            }
            if filtered.len() >= MAX_MERGED_SUGGESTIONS {
                break;
            }
        }

        if saw_kind_metadata {
            filtered
        } else {
            suggestions
        }
    }

    fn collect_expected_object_suggestions(
        data: &mut IntellisenseData,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        match Self::expected_object_suggestion_kind(prefix, None, deep_ctx) {
            Some(kind) => Self::collect_expected_object_suggestions_for_kind(data, prefix, kind),
            None => Vec::new(),
        }
    }

    // Exercised by unit tests; not yet wired into a production code path.
    #[allow(dead_code)]
    fn table_context_expected_object_suggestions(
        data: &mut IntellisenseData,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Option<Vec<String>> {
        let kind = Self::expected_object_suggestion_kind(prefix, None, deep_ctx)?;
        Some(Self::collect_expected_object_suggestions_for_kind(
            data, prefix, kind,
        ))
    }

    fn expand_virtual_table_wildcards(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        let wildcard_tables = intellisense_context::extract_select_list_wildcard_scopes(
            body_tokens,
            body_tables_in_scope,
        );
        if wildcard_tables.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let wildcard_tables = Self::effective_wildcard_column_tables(
            wildcard_tables,
            body_tables_in_scope,
            virtual_table_columns,
        );
        let mut wildcard_columns = Vec::new();
        for table in &wildcard_tables {
            if Self::virtual_table_columns_for_lookup(virtual_table_columns, table).is_none() {
                Self::request_table_columns(table, intellisense_data, column_sender, connection);
            }
            let columns = Self::columns_for_virtual_or_cached_table(
                table,
                virtual_table_columns,
                intellisense_data,
            );
            wildcard_columns.extend(columns);
        }
        Self::dedup_column_names_case_insensitive(&mut wildcard_columns);
        (wildcard_columns, wildcard_tables)
    }

    fn effective_wildcard_column_tables(
        wildcard_tables: Vec<String>,
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        wildcard_tables
            .into_iter()
            .map(|table| {
                if Self::virtual_table_columns_for_lookup(virtual_table_columns, &table).is_some() {
                    return table;
                }
                if let Some(source) = body_tables_in_scope.iter().find_map(|table_ref| {
                    table_ref
                        .alias
                        .as_deref()
                        .filter(|alias| alias.eq_ignore_ascii_case(&table))
                        .map(|_| table_ref.name.clone())
                }) {
                    return source;
                }
                body_tables_in_scope
                    .iter()
                    .find_map(|table_ref| {
                        let alias = table_ref.alias.as_deref()?;
                        (table_ref.name.eq_ignore_ascii_case(&table)
                            && Self::virtual_table_columns_for_lookup(virtual_table_columns, alias)
                                .is_some())
                        .then(|| alias.to_string())
                    })
                    .unwrap_or(table)
            })
            .collect()
    }

    fn columns_for_virtual_or_cached_table(
        table: &str,
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
    ) -> Vec<String> {
        if let Some(columns) = Self::virtual_table_columns_for_lookup(virtual_table_columns, table) {
            return columns.to_vec();
        }

        let data = intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.get_columns_for_table(table)
    }

    fn dedup_column_names_case_insensitive(columns: &mut Vec<String>) {
        let mut seen = HashSet::new();
        columns.retain(|column| seen.insert(Self::completion_identifier_lookup_upper(column)));
    }

    fn column_sets_match_case_insensitive(left: &[String], right: &[String]) -> bool {
        if left.len() != right.len() {
            return false;
        }

        let mut left_keys: Vec<String> = left
            .iter()
            .map(|column| Self::completion_identifier_lookup_upper(column))
            .collect();
        let mut right_keys: Vec<String> = right
            .iter()
            .map(|column| Self::completion_identifier_lookup_upper(column))
            .collect();
        left_keys.sort_unstable();
        right_keys.sort_unstable();
        left_keys == right_keys
    }

    fn dedup_local_member_entries_case_insensitive(entries: &mut Vec<LocalMemberEntry>) {
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert(entry.upper.clone()));
    }

    /// Column qualifier to use in generated SQL: the alias if present,
    /// otherwise the table's unqualified (short) name.
    fn auto_join_qualifier(table: &intellisense_context::ScopedTableRef) -> String {
        if let Some(alias) = table.alias.as_deref() {
            if !alias.is_empty() {
                return alias.to_string();
            }
        }

        Self::auto_join_relation_segments(&table.name)
            .and_then(|segments| {
                segments.last().map(|segment| {
                    if segment.quoted_dotted {
                        Self::quote_identifier_segment_for_completion(&segment.text)
                    } else {
                        segment.text.clone()
                    }
                })
            })
            .unwrap_or_else(|| Self::strip_identifier_quotes(&table.name))
    }

    /// Compare two table references by their unquoted short (unqualified) name.
    fn auto_join_tables_match(a: &str, b: &str) -> bool {
        let Some(a_key) = Self::auto_join_table_match_key(a) else {
            return false;
        };
        let Some(b_key) = Self::auto_join_table_match_key(b) else {
            return false;
        };

        if a_key.full == b_key.full {
            return true;
        }

        a_key.allow_short_match && b_key.allow_short_match && a_key.short == b_key.short
    }

    fn auto_join_table_match_key(value: &str) -> Option<AutoJoinTableMatchKey> {
        let segments = Self::auto_join_relation_segments(value)?;
        let last = segments.last()?;
        let full = segments
            .iter()
            .map(Self::auto_join_relation_segment_match_key)
            .collect::<Vec<_>>()
            .join(".");
        let short = Self::auto_join_relation_segment_match_key(last);

        Some(AutoJoinTableMatchKey {
            full,
            short,
            allow_short_match: !last.quoted_dotted,
        })
    }

    fn auto_join_relation_segment_match_key(segment: &AutoJoinRelationSegment) -> String {
        let kind = if segment.quoted_dotted { "Q" } else { "U" };
        format!("{kind}:{}", segment.text.to_ascii_uppercase())
    }

    fn auto_join_relation_segments(value: &str) -> Option<Vec<AutoJoinRelationSegment>> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;

        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
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
                    segments.push(Self::auto_join_relation_segment(current.trim())?);
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        if active_quote.is_some() {
            return None;
        }

        segments.push(Self::auto_join_relation_segment(current.trim())?);
        Some(segments)
    }

    fn auto_join_relation_segment(raw: &str) -> Option<AutoJoinRelationSegment> {
        if raw.trim().is_empty() {
            return None;
        }

        let text = Self::strip_identifier_quotes(raw);
        if text.trim().is_empty() {
            return None;
        }

        Some(AutoJoinRelationSegment {
            quoted_dotted: sql_text::is_quoted_identifier(raw) && text.contains('.'),
            text,
        })
    }

    fn format_auto_join_pairs(
        left_q: &str,
        left_cols: &[String],
        right_q: &str,
        right_cols: &[String],
    ) -> String {
        let left_scope = Self::render_select_list_wildcard_scope(left_q);
        let right_scope = Self::render_select_list_wildcard_scope(right_q);
        left_cols
            .iter()
            .zip(right_cols)
            .map(|(left, right)| {
                format!(
                    "{}.{} = {}.{}",
                    left_scope,
                    Self::quote_identifier_segment_for_completion(left),
                    right_scope,
                    Self::quote_identifier_segment_for_completion(right)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Build an FK-based join condition (e.g. `e.DEPTNO = d.DEPTNO`) between the
    /// most-recently joined table (`right`) and an earlier table in scope, when
    /// a foreign key relates them in either direction. Returns the condition
    /// expression without the `ON` keyword. Prefers the nearest preceding table.
    fn build_auto_join_condition(
        data: &IntellisenseData,
        right: &intellisense_context::ScopedTableRef,
        lefts: &[&intellisense_context::ScopedTableRef],
    ) -> Option<String> {
        let right_q = Self::auto_join_qualifier(right);
        for left in lefts.iter().rev() {
            let left_q = Self::auto_join_qualifier(left);

            if let Some(fks) = data.get_foreign_keys(&right.name) {
                for fk in fks {
                    if Self::auto_join_tables_match(&fk.ref_table, &left.name)
                        && !fk.columns.is_empty()
                        && fk.columns.len() == fk.ref_columns.len()
                    {
                        return Some(Self::format_auto_join_pairs(
                            &right_q,
                            &fk.columns,
                            &left_q,
                            &fk.ref_columns,
                        ));
                    }
                }
            }

            if let Some(fks) = data.get_foreign_keys(&left.name) {
                for fk in fks {
                    if Self::auto_join_tables_match(&fk.ref_table, &right.name)
                        && !fk.columns.is_empty()
                        && fk.columns.len() == fk.ref_columns.len()
                    {
                        return Some(Self::format_auto_join_pairs(
                            &left_q,
                            &fk.columns,
                            &right_q,
                            &fk.ref_columns,
                        ));
                    }
                }
            }
        }
        None
    }

    /// Build the display-detail map (column name upper -> "TYPE  PK/NN  FK→T")
    /// for every column of the in-scope tables. First occurrence of a column
    /// name wins, matching how column suggestions are de-duplicated.
    /// Build the popup's per-suggestion detail map (type column + PK/NN/FK
    /// badge column). Column entries carry their data type and constraint
    /// badges; every other entry (table, view, keyword, …) gets a type label
    /// via [`IntellisenseData::suggestion_type_label`]. Column details take
    /// precedence over object-store labels for a shared name.
    fn build_suggestion_details(
        data: &IntellisenseData,
        suggestions: &[String],
        column_tables: &[String],
        db_type: Option<crate::db::DatabaseType>,
    ) -> HashMap<String, SuggestionDetail> {
        let mut details = Self::build_column_descriptions(data, column_tables);
        for suggestion in suggestions {
            let key = Self::completion_identifier_lookup_upper(suggestion);
            if details.contains_key(&key) {
                continue;
            }
            if let Some(label) = data.suggestion_type_label(suggestion, db_type) {
                details.insert(
                    key,
                    SuggestionDetail {
                        type_text: label.to_string(),
                        badges: String::new(),
                    },
                );
            }
        }
        details
    }

    fn build_column_descriptions(
        data: &IntellisenseData,
        column_tables: &[String],
    ) -> HashMap<String, SuggestionDetail> {
        let mut descriptions: HashMap<String, SuggestionDetail> = HashMap::new();
        for table in column_tables {
            let mut fk_targets: HashMap<String, String> = HashMap::new();
            if let Some(fks) = data.get_foreign_keys(table) {
                for fk in fks {
                    for column in &fk.columns {
                        fk_targets
                            .entry(Self::completion_identifier_lookup_upper(column))
                            .or_insert_with(|| fk.ref_table.clone());
                    }
                }
            }

            for column in data.get_columns_for_table(table) {
                let key = Self::completion_identifier_lookup_upper(&column);
                if descriptions.contains_key(&key) {
                    continue;
                }
                let Some(meta) = data.get_column_meta(table, &column) else {
                    continue;
                };

                let mut badges = String::new();
                if meta.is_primary_key {
                    badges.push_str("PK");
                } else if !meta.nullable {
                    badges.push_str("NN");
                }
                if let Some(ref_table) = fk_targets.get(&key) {
                    if !badges.is_empty() {
                        badges.push_str("  ");
                    }
                    badges.push_str(&format!("FK\u{2192}{ref_table}"));
                }

                if !meta.type_display.trim().is_empty() || !badges.is_empty() {
                    descriptions.insert(
                        key,
                        SuggestionDetail {
                            type_text: meta.type_display.clone(),
                            badges,
                        },
                    );
                }
            }
        }
        descriptions
    }

    fn has_column_loading_for_scope(
        include_columns: bool,
        column_tables: &[String],
        virtual_wildcard_dependencies: &HashMap<String, Vec<String>>,
        data: &IntellisenseData,
    ) -> bool {
        if !include_columns {
            return false;
        }

        fn table_is_loading(data: &IntellisenseData, table: &str) -> bool {
            if let Some(key) = SqlEditorWidget::resolve_table_column_load_key(data, table) {
                if data.columns_loading.contains(&key.to_uppercase()) {
                    return true;
                }
            }

            let upper = table.to_uppercase();
            if data.columns_loading.contains(&upper) {
                return true;
            }
            // Only build full candidate list when the name has a qualified dot.
            if !SqlEditorWidget::has_unquoted_dot(table) {
                return false;
            }
            SqlEditorWidget::table_lookup_key_candidates(table)
                .iter()
                .any(|key| {
                    let key_upper = key.to_uppercase();
                    key_upper != upper && data.columns_loading.contains(&key_upper)
                })
        }

        column_tables.iter().any(|table| {
            if table_is_loading(data, table) {
                return true;
            }
            let key = table.to_uppercase();
            virtual_wildcard_dependencies
                .get(&key)
                .is_some_and(|deps| deps.iter().any(|dep| table_is_loading(data, dep)))
        })
    }

    fn collect_context_name_suggestions(
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
        context: SqlContext,
    ) -> Vec<String> {
        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        let allow_relation_aliases = !matches!(context, SqlContext::TableName);

        let mut push_candidate = |candidate: &str| {
            if candidate.is_empty() {
                return;
            }
            let candidate_upper = Self::completion_identifier_lookup_upper(candidate);
            if !prefix_upper.is_empty() && !candidate_upper.starts_with(&prefix_upper) {
                return;
            }
            if seen.insert(candidate_upper) {
                suggestions.push(candidate.to_string());
            }
        };

        if allow_relation_aliases {
            for table_ref in &deep_ctx.tables_in_scope {
                if let Some(alias) = table_ref.alias.as_deref() {
                    push_candidate(alias);
                }
            }
        }

        for cte in &deep_ctx.ctes {
            push_candidate(&cte.name);
        }

        if allow_relation_aliases {
            for subq in &deep_ctx.subqueries {
                push_candidate(&subq.alias);
            }
        }

        suggestions
    }

    /// Where the unqualified select-list wildcards (`*`, `t.*`) are grammatical
    /// relative to the cursor's paren nesting. They name a whole projection, so
    /// they belong only at a query's own select-list level — the top level, or the
    /// select list of a subquery whose `(` directly encloses the cursor — never as
    /// a value inside a function-call / expression sub-paren (`f(…, |)`, `OVER
    /// (PARTITION BY |)`), where the only surviving form is the bare `*` of an
    /// aggregate `COUNT(*)` (cursor right after `(`). Distinguishing a subquery
    /// paren from a function paren is what keeps `*`/`t.*` flowing into a nested
    /// `… IN (SELECT | …)` while dropping the `emp.*` noise that leaked into every
    /// `f(… , |)` argument.
    fn select_list_wildcard_slot(tokens: &[SqlToken], end: usize) -> SelectListWildcardSlot {
        // Find the innermost `(` still open at the cursor.
        let mut depth = 0i32;
        let mut open_idx = None;
        for idx in (0..end.min(tokens.len())).rev() {
            match &tokens[idx] {
                SqlToken::Symbol(sym) if sym == ")" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" => {
                    if depth == 0 {
                        open_idx = Some(idx);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let Some(open_idx) = open_idx else {
            // No enclosing paren: the statement's own select list. `*`/`t.*` name
            // a whole-row projection, so they belong only at the start of a fresh
            // select-list item — never mid-expression (a `CASE` arm, an operator
            // operand), where they would be ungrammatical noise.
            return if Self::cursor_at_select_projection_item_start(tokens, end) {
                SelectListWildcardSlot::Full
            } else {
                SelectListWildcardSlot::None
            };
        };
        if intellisense_context::is_query_expression_start(tokens, open_idx + 1) {
            // A subquery paren — the cursor is in that subquery's select list, so
            // the same projection-item-start requirement applies.
            return if Self::cursor_at_select_projection_item_start(tokens, end) {
                SelectListWildcardSlot::Full
            } else {
                SelectListWildcardSlot::None
            };
        }
        // A function-call / expression paren: the only surviving wildcard is the
        // bare `*` argument of `COUNT(*)` — the one function that admits it — with
        // the cursor sitting immediately after the `(`. Any other call
        // (`SUM(|)`, `NVL(|, 0)`, `TRIM(|)`) takes a value expression, never `*`.
        let cursor_right_after_open = Self::meaningful_tokens_before(tokens, end).last().is_some_and(
            |token| matches!(token, SqlToken::Symbol(sym) if sym == "("),
        );
        let enclosing_call_is_count = Self::innermost_open_paren_preceding_word(tokens, end)
            .is_some_and(|word| word.eq_ignore_ascii_case("COUNT"));
        if cursor_right_after_open && enclosing_call_is_count {
            SelectListWildcardSlot::CountStarOnly
        } else {
            SelectListWildcardSlot::None
        }
    }

    /// True when the cursor sits at the start of a fresh select-list projection
    /// item — right after `SELECT`, a set-quantifier (`DISTINCT`/`ALL`/`UNIQUE`/
    /// `DISTINCTROW`), or a list-separating comma. A whole-row wildcard (`*`/
    /// `t.*`) is itself a projection item, so it is grammatical only here;
    /// anywhere else inside a select-list expression (a `CASE` arm such as
    /// `CASE WHEN |`/`THEN |`/`ELSE |`, or an operator operand) it is noise.
    fn cursor_at_select_projection_item_start(tokens: &[SqlToken], end: usize) -> bool {
        match Self::meaningful_tokens_before(tokens, end).last() {
            Some(SqlToken::Word(word)) => matches!(
                word.to_ascii_uppercase().as_str(),
                "SELECT" | "DISTINCT" | "ALL" | "UNIQUE" | "DISTINCTROW"
            ),
            Some(SqlToken::Symbol(sym)) => sym == ",",
            Some(_) => false,
            // Nothing precedes the cursor's word in this query window: the cursor
            // is at the very start of the (sub)query's select list — a projection
            // item start.
            None => true,
        }
    }

    fn collect_clause_wildcard_suggestions(
        prefix: &str,
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let policy = ClauseCompletionPolicy::for_phase(deep_ctx.phase, qualifier.is_some());
        let prefix_upper = Self::completion_wildcard_candidate_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();

        let mut push_candidate = |candidate: String| {
            if candidate.is_empty() {
                return;
            }
            let candidate_upper = Self::completion_wildcard_candidate_lookup_upper(&candidate);
            if !prefix_upper.is_empty() && !candidate_upper.starts_with(prefix_upper.as_str()) {
                return;
            }
            if seen.insert(candidate_upper) {
                suggestions.push(candidate);
            }
        };

        match policy.select_list_wildcard_mode {
            SelectListWildcardMode::None => {}
            SelectListWildcardMode::Qualified => {
                // `t.*` is grammatical only when `t` actually names a row source in
                // scope (a table/alias/CTE/subquery in this query). For an
                // unresolved qualifier (`x.|` where `x` is a typo or not in the
                // `FROM`) the column completion already yields nothing, so a lone
                // `*` would be the only — and bogus — suggestion. The scope test is
                // purely structural (parsed `FROM`/CTE/subquery), so it never hides
                // `t.*` while the table's columns are still loading.
                if qualifier
                    .is_some_and(|q| Self::qualifier_matches_visible_relation_scope(q, deep_ctx))
                {
                    push_candidate("*".to_string());
                }
            }
            SelectListWildcardMode::Unqualified => {
                let current_query_tokens = Self::current_query_tokens(deep_ctx);
                let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
                let end = Self::expected_suggestion_context_end(
                    current_query_tokens,
                    cursor_token_len,
                    !prefix.is_empty(),
                );
                match Self::select_list_wildcard_slot(current_query_tokens, end) {
                    SelectListWildcardSlot::None => {}
                    SelectListWildcardSlot::CountStarOnly => {
                        push_candidate("*".to_string());
                    }
                    SelectListWildcardSlot::Full => {
                        push_candidate("*".to_string());
                        let current_query_tables =
                            intellisense_context::collect_tables_in_statement(current_query_tokens);
                        for table_ref in current_query_tables {
                            let scope_name = table_ref
                                .alias
                                .as_deref()
                                .unwrap_or(table_ref.name.as_str());
                            let rendered_scope =
                                Self::render_select_list_wildcard_scope(scope_name);
                            if !rendered_scope.is_empty() {
                                push_candidate(format!("{rendered_scope}.*"));
                            }
                        }
                    }
                }
            }
        }

        suggestions.truncate(MAX_MERGED_SUGGESTIONS);
        suggestions
    }

    fn merge_suggestions_with_context_aliases(
        mut base: Vec<String>,
        aliases: Vec<String>,
        prefer_aliases: bool,
    ) -> Vec<String> {
        if aliases.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let mut seen: HashSet<String> = base
            .iter()
            .map(|item| Self::completion_identifier_lookup_upper(item))
            .collect();
        let mut filtered_aliases = Vec::new();
        for alias in aliases {
            if seen.insert(Self::completion_identifier_lookup_upper(&alias)) {
                filtered_aliases.push(alias);
            }
        }

        if filtered_aliases.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let mut merged = if prefer_aliases {
            filtered_aliases.extend(base);
            filtered_aliases
        } else {
            base.extend(filtered_aliases);
            base
        };
        merged.truncate(MAX_MERGED_SUGGESTIONS);
        merged
    }

    fn qualified_condition_comparison_precedes_base_suggestions(
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        matches!(phase, intellisense_context::SqlPhase::JoinCondition)
    }

    fn merge_qualified_condition_comparison_suggestions(
        base: Vec<String>,
        comparisons: Vec<String>,
        phase: intellisense_context::SqlPhase,
    ) -> Vec<String> {
        Self::merge_suggestions_with_context_aliases(
            base,
            comparisons,
            Self::qualified_condition_comparison_precedes_base_suggestions(phase),
        )
    }

    fn maybe_merge_suggestions_with_context_aliases(
        mut base: Vec<String>,
        aliases: Vec<String>,
        prefer_aliases: bool,
        has_qualifier: bool,
    ) -> Vec<String> {
        if has_qualifier {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }
        Self::merge_suggestions_with_context_aliases(base, aliases, prefer_aliases)
    }

    fn infer_columns_from_partial_select_qualifiers(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> Vec<String> {
        let qualifiers = intellisense_context::extract_select_list_leading_qualifiers(body_tokens);
        if qualifiers.is_empty() {
            return Vec::new();
        }

        let mut columns = Vec::new();
        for qualifier in qualifiers {
            let mut tables =
                intellisense_context::resolve_qualifier_tables(&qualifier, body_tables_in_scope);
            let unresolved_direct =
                tables.len() == 1 && tables[0].eq_ignore_ascii_case(qualifier.as_str());
            if unresolved_direct {
                let outer_tables = intellisense_context::resolve_qualifier_tables(
                    &qualifier,
                    outer_tables_in_scope,
                );
                let outer_unresolved_direct = outer_tables.len() == 1
                    && outer_tables[0].eq_ignore_ascii_case(qualifier.as_str());
                if !outer_unresolved_direct {
                    tables = outer_tables;
                }
            }

            for table in tables {
                if let Some(virtual_cols) =
                    Self::virtual_table_columns_for_lookup(virtual_table_columns, &table)
                {
                    columns.extend(virtual_cols.iter().cloned());
                    continue;
                }

                let mut table_columns = {
                    let data = intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    data.get_columns_for_table(&table)
                };
                if table_columns.is_empty() {
                    Self::request_table_columns(
                        &table,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                    table_columns = {
                        let data = intellisense_data
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        data.get_columns_for_table(&table)
                    };
                }
                columns.extend(table_columns);
            }
        }

        Self::dedup_column_names_case_insensitive(&mut columns);
        columns
    }

    fn build_virtual_table_columns_for_query_body(
        body_tokens: &[SqlToken],
        seed_virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> HashMap<String, Vec<String>> {
        let body_ctx = intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
        let mut virtual_table_columns = seed_virtual_table_columns.clone();

        for cte in &body_ctx.ctes {
            let (columns, _) = Self::collect_cte_virtual_columns_for_completion(
                &body_ctx,
                cte,
                &virtual_table_columns,
                intellisense_data,
                column_sender,
                connection,
            );
            if !columns.is_empty() {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &cte.name, columns);
            }
        }

        for subq in &body_ctx.subqueries {
            if let Some(columns) = Self::explicit_subquery_columns_for_completion(&body_ctx, subq) {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &subq.alias, columns);
                continue;
            }
            let relation_tokens = intellisense_context::token_range_slice(
                body_ctx.statement_tokens.as_ref(),
                subq.body_range,
            );
            let relation_ctx = intellisense_context::analyze_cursor_context(
                relation_tokens,
                relation_tokens.len(),
            );
            let mut relation_virtual_table_columns = virtual_table_columns.clone();

            for cte in &relation_ctx.ctes {
                let (columns, _) = Self::collect_cte_virtual_columns_for_completion(
                    &relation_ctx,
                    cte,
                    &relation_virtual_table_columns,
                    intellisense_data,
                    column_sender,
                    connection,
                );
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut relation_virtual_table_columns,
                        &cte.name,
                        columns,
                    );
                }
            }

            let relation_local_tables =
                intellisense_context::collect_tables_in_statement(relation_tokens);
            let (columns, _) = Self::collect_virtual_relation_columns_for_completion(
                relation_tokens,
                &relation_local_tables,
                &body_ctx.tables_in_scope,
                &relation_virtual_table_columns,
                intellisense_data,
                column_sender,
                connection,
            );
            if !columns.is_empty() {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &subq.alias, columns);
            }
        }

        virtual_table_columns
    }

    fn collect_virtual_query_projection_columns(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        let available_virtual_table_columns = Self::build_virtual_table_columns_for_query_body(
            body_tokens,
            virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        );
        let pivot_unpivot_columns =
            intellisense_context::extract_oracle_pivot_unpivot_projection_columns(body_tokens);
        let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
        let mut use_pivot_unpivot_projection =
            columns.is_empty() && !pivot_unpivot_columns.is_empty();
        if !columns.is_empty() && !pivot_unpivot_columns.is_empty() {
            let pivot_unpivot_source_columns =
                intellisense_context::extract_oracle_pivot_unpivot_source_projection_columns(
                    body_tokens,
                );
            if Self::column_sets_match_case_insensitive(&columns, &pivot_unpivot_source_columns) {
                columns.clear();
                use_pivot_unpivot_projection = true;
            }
        }
        if columns.is_empty() {
            columns = intellisense_context::extract_table_function_columns(body_tokens);
        }
        columns.extend(Self::infer_columns_from_partial_select_qualifiers(
            body_tokens,
            body_tables_in_scope,
            outer_tables_in_scope,
            &available_virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        ));

        let (wildcard_columns, wildcard_tables) = Self::expand_virtual_table_wildcards(
            body_tokens,
            body_tables_in_scope,
            &available_virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        );
        columns.extend(wildcard_columns);
        if use_pivot_unpivot_projection {
            columns.extend(pivot_unpivot_columns);
        }
        columns.extend(intellisense_context::extract_oracle_model_generated_columns(body_tokens));
        columns
            .extend(intellisense_context::extract_match_recognize_generated_columns(body_tokens));
        Self::dedup_column_names_case_insensitive(&mut columns);
        (columns, wildcard_tables)
    }

    fn collect_virtual_relation_columns_for_completion(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        Self::collect_virtual_query_projection_columns(
            body_tokens,
            body_tables_in_scope,
            outer_tables_in_scope,
            virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        )
    }

    fn collect_common_column_suggestions(
        prefix: &str,
        column_tables: &[String],
        data: &IntellisenseData,
    ) -> Vec<String> {
        if column_tables.len() < 2 {
            return Vec::new();
        }

        let mut iter = column_tables.iter();
        let Some(first_table) = iter.next() else {
            return Vec::new();
        };
        let mut common_columns: Vec<(String, String)> = data
            .get_columns_for_table(first_table)
            .into_iter()
            .map(|column| {
                let upper = Self::completion_identifier_lookup_upper(&column);
                (column, upper)
            })
            .collect();
        if common_columns.is_empty() {
            return Vec::new();
        }

        for table in iter {
            let table_columns = data.get_columns_for_table(table);
            if table_columns.is_empty() {
                return Vec::new();
            }
            let allowed: HashSet<String> = table_columns
                .into_iter()
                .map(|column| Self::completion_identifier_lookup_upper(&column))
                .collect();
            common_columns
                .retain(|(_, upper)| allowed.contains(upper));
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        for (column, upper) in common_columns {
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                suggestions.push(column);
                if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                    break;
                }
            }
        }
        suggestions
    }

    fn current_query_tokens(deep_ctx: &intellisense_context::CursorContext) -> &[SqlToken] {
        deep_ctx
            .active_query_range
            .map(|range| {
                intellisense_context::token_range_slice(deep_ctx.statement_tokens.as_ref(), range)
            })
            .unwrap_or_else(|| deep_ctx.statement_tokens.as_ref())
    }

    fn cursor_token_len_in_current_query(deep_ctx: &intellisense_context::CursorContext) -> usize {
        deep_ctx
            .active_query_range
            .map(|range| {
                deep_ctx
                    .cursor_token_len
                    .saturating_sub(range.start)
                    .min(range.end.saturating_sub(range.start))
            })
            .unwrap_or(deep_ctx.cursor_token_len)
    }

    fn next_word_upper_in_tokens(tokens: &[SqlToken], idx: usize) -> Option<(String, usize)> {
        let mut current_idx = idx;
        while current_idx < tokens.len() {
            match &tokens[current_idx] {
                SqlToken::Comment(_) => current_idx += 1,
                SqlToken::Word(word) => return Some((word.to_ascii_uppercase(), current_idx)),
                _ => return None,
            }
        }
        None
    }

    fn cursor_is_in_query_level_order_by(deep_ctx: &intellisense_context::CursorContext) -> bool {
        if !matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause
        ) {
            return false;
        }

        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let token_depths = crate::ui::sql_depth::paren_depths(current_query_tokens);
        let mut idx = 0usize;
        let limit = cursor_token_len.min(current_query_tokens.len());

        while idx < limit {
            if !crate::ui::sql_depth::is_top_level_depth(&token_depths, idx) {
                idx += 1;
                continue;
            }

            let SqlToken::Word(word) = &current_query_tokens[idx] else {
                idx += 1;
                continue;
            };
            if !word.eq_ignore_ascii_case("ORDER") {
                idx += 1;
                continue;
            }

            let Some((next_keyword, next_idx)) =
                Self::next_word_upper_in_tokens(current_query_tokens, idx + 1)
            else {
                return false;
            };

            if next_keyword == "BY" && next_idx < limit {
                return true;
            }

            if next_keyword == "SIBLINGS" {
                if let Some((tail_keyword, tail_idx)) =
                    Self::next_word_upper_in_tokens(current_query_tokens, next_idx + 1)
                {
                    if tail_keyword == "BY" && tail_idx < limit {
                        return true;
                    }
                }
            }

            idx += 1;
        }

        false
    }

    fn virtual_table_columns_for_lookup<'a>(
        virtual_table_columns: &'a HashMap<String, Vec<String>>,
        table: &str,
    ) -> Option<&'a [String]> {
        let candidates = Self::table_lookup_key_candidates(table);
        for candidate in &candidates {
            let key = Self::completion_identifier_lookup_upper(candidate);
            if let Some(columns) = virtual_table_columns.get(&key) {
                return Some(columns.as_slice());
            }
        }

        let key = Self::completion_identifier_lookup_upper(table);
        virtual_table_columns.get(&key).map(|cols| cols.as_slice())
    }

    fn insert_virtual_table_columns(
        virtual_table_columns: &mut HashMap<String, Vec<String>>,
        relation_name: &str,
        columns: Vec<String>,
    ) {
        virtual_table_columns.insert(Self::completion_identifier_lookup_upper(relation_name), columns);
    }

    fn is_cursor_inside_subquery_explicit_column_list(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> bool {
        let cursor_token_idx = deep_ctx
            .cursor_token_len
            .min(deep_ctx.statement_tokens.len());
        subquery
            .explicit_column_range
            .is_some_and(|range| cursor_token_idx >= range.start && cursor_token_idx <= range.end)
    }

    fn explicit_subquery_columns_for_completion(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> Option<Vec<String>> {
        if subquery.explicit_columns.is_empty()
            || Self::is_cursor_inside_subquery_explicit_column_list(deep_ctx, subquery)
        {
            return None;
        }

        let mut columns = subquery.explicit_columns.clone();
        Self::dedup_column_names_case_insensitive(&mut columns);
        (!columns.is_empty()).then_some(columns)
    }

    fn virtual_subquery_replaces_source_columns(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> bool {
        if Self::explicit_subquery_columns_for_completion(deep_ctx, subquery).is_some() {
            return true;
        }

        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            subquery.body_range,
        );
        !intellisense_context::extract_oracle_pivot_unpivot_projection_columns(body_tokens)
            .is_empty()
    }

    fn column_lookup_table_for_table_ref(
        table_ref: &intellisense_context::ScopedTableRef,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> String {
        if let Some(alias) = table_ref.alias.as_deref() {
            if let Some(subquery) = deep_ctx
                .subqueries
                .iter()
                .find(|subquery| subquery.alias.eq_ignore_ascii_case(alias))
            {
                if Self::virtual_subquery_replaces_source_columns(deep_ctx, subquery) {
                    return subquery.alias.clone();
                }
            }
        }

        table_ref.name.clone()
    }

    fn resolve_all_scope_column_lookup_tables(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let mut ordered_tables: Vec<(usize, &intellisense_context::ScopedTableRef)> =
            deep_ctx.tables_in_scope.iter().enumerate().collect();
        ordered_tables.sort_by(|(left_idx, left), (right_idx, right)| {
            right
                .depth
                .cmp(&left.depth)
                .then_with(|| left_idx.cmp(right_idx))
        });

        let mut tables = Vec::new();
        let mut seen = HashSet::new();

        for (_, table_ref) in ordered_tables {
            let lookup_table = Self::column_lookup_table_for_table_ref(table_ref, deep_ctx);
            let key = Self::completion_identifier_lookup_upper(&lookup_table);
            if seen.insert(key) {
                tables.push(lookup_table);
            }
        }

        tables
    }

    fn resolve_focused_column_lookup_tables(
        focused_tables: &[String],
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let mut tables = Vec::new();
        let mut seen = HashSet::new();

        for focused in focused_tables {
            let mut matched = false;
            for table_ref in &deep_ctx.tables_in_scope {
                let matches_focused = table_ref.name.eq_ignore_ascii_case(focused)
                    || table_ref
                        .alias
                        .as_deref()
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(focused));
                if !matches_focused {
                    continue;
                }

                matched = true;
                let lookup_table = Self::column_lookup_table_for_table_ref(table_ref, deep_ctx);
                let key = Self::completion_identifier_lookup_upper(&lookup_table);
                if seen.insert(key) {
                    tables.push(lookup_table);
                }
            }

            if !matched {
                let key = Self::completion_identifier_lookup_upper(focused);
                if seen.insert(key) {
                    tables.push(focused.clone());
                }
            }
        }

        tables
    }

    fn resolve_column_tables_for_context(
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        fn virtual_alias_for_qualifier<'a>(
            qualifier: &str,
            deep_ctx: &'a intellisense_context::CursorContext,
        ) -> Option<&'a intellisense_context::SubqueryDefinition> {
            deep_ctx
                .subqueries
                .iter()
                .find(|subq| subq.alias.eq_ignore_ascii_case(qualifier))
        }

        fn replacement_virtual_alias_scope(
            qualifier: &str,
            deep_ctx: &intellisense_context::CursorContext,
        ) -> Option<Vec<String>> {
            let subquery = virtual_alias_for_qualifier(qualifier, deep_ctx)?;
            SqlEditorWidget::virtual_subquery_replaces_source_columns(deep_ctx, subquery)
                .then(|| vec![subquery.alias.clone()])
        }

        fn prepend_virtual_alias_if_present(
            tables: &mut Vec<String>,
            qualifier: &str,
            deep_ctx: &intellisense_context::CursorContext,
        ) {
            let Some(subquery) = virtual_alias_for_qualifier(qualifier, deep_ctx) else {
                return;
            };
            let alias = subquery.alias.clone();

            if tables
                .iter()
                .any(|table| table.eq_ignore_ascii_case(&alias))
            {
                if let Some(existing_idx) = tables
                    .iter()
                    .position(|table| table.eq_ignore_ascii_case(&alias))
                {
                    if existing_idx != 0 {
                        let existing = tables.remove(existing_idx);
                        tables.insert(0, existing);
                    }
                }
                return;
            }

            tables.insert(0, alias);
        }

        let focused_tables =
            (!deep_ctx.focused_tables.is_empty()).then_some(&deep_ctx.focused_tables);
        if qualifier.is_some()
            && matches!(
                deep_ctx.phase,
                intellisense_context::SqlPhase::JoinUsingColumnList
            )
        {
            return Vec::new();
        }
        let Some(qualifier) = qualifier else {
            if let Some(focused_tables) = focused_tables {
                if matches!(
                    deep_ctx.phase,
                    intellisense_context::SqlPhase::LockingColumnList
                ) {
                    return Self::resolve_focused_column_lookup_tables(focused_tables, deep_ctx);
                }
                return focused_tables.to_vec();
            }
            return Self::resolve_all_scope_column_lookup_tables(deep_ctx);
        };

        let resolved =
            intellisense_context::resolve_qualifier_tables(qualifier, &deep_ctx.tables_in_scope);
        if let Some(virtual_scope) = replacement_virtual_alias_scope(qualifier, deep_ctx) {
            return virtual_scope;
        }
        if let Some(focused_tables) = focused_tables {
            let filtered: Vec<String> = resolved
                .iter()
                .filter(|name| {
                    focused_tables
                        .iter()
                        .any(|focused| focused.eq_ignore_ascii_case(name))
                })
                .cloned()
                .collect();
            if !filtered.is_empty() {
                let mut filtered = filtered;
                prepend_virtual_alias_if_present(&mut filtered, qualifier, deep_ctx);
                return filtered;
            }
        }
        let unresolved_direct = resolved.len() == 1 && resolved[0].eq_ignore_ascii_case(qualifier);
        if !unresolved_direct {
            if focused_tables.is_some() {
                return Vec::new();
            }
            let mut resolved = resolved;
            prepend_virtual_alias_if_present(&mut resolved, qualifier, deep_ctx);
            return resolved;
        }

        let pattern_vars = intellisense_context::extract_match_recognize_pattern_variables(
            Self::current_query_tokens(deep_ctx),
        );
        if pattern_vars
            .iter()
            .any(|var| var.eq_ignore_ascii_case(qualifier))
        {
            return intellisense_context::resolve_all_scope_tables(&deep_ctx.tables_in_scope);
        }

        let mut resolved = resolved;
        prepend_virtual_alias_if_present(&mut resolved, qualifier, deep_ctx);
        resolved
    }

    fn token_is_qualified_identifier_segment(token: &SqlToken) -> bool {
        matches!(token, SqlToken::Word(_) | SqlToken::String(_))
    }

    fn current_qualified_identifier_chain_start(
        tokens: &[SqlToken],
        cursor_token_len: usize,
    ) -> Option<usize> {
        if cursor_token_len == 0 || cursor_token_len > tokens.len() {
            return None;
        }

        let mut start = cursor_token_len;
        if start >= 2
            && matches!(tokens.get(start - 1), Some(SqlToken::Symbol(symbol)) if symbol == ".")
            && tokens
                .get(start - 2)
                .is_some_and(Self::token_is_qualified_identifier_segment)
        {
            start -= 2;
        } else if tokens
            .get(start - 1)
            .is_some_and(Self::token_is_qualified_identifier_segment)
        {
            start -= 1;
            while start >= 2
                && matches!(tokens.get(start - 1), Some(SqlToken::Symbol(symbol)) if symbol == ".")
                && tokens
                    .get(start - 2)
                    .is_some_and(Self::token_is_qualified_identifier_segment)
            {
                start -= 2;
            }
        } else {
            return None;
        }

        Some(start)
    }

    fn previous_non_comment_token(tokens: &[SqlToken], end: usize) -> Option<&SqlToken> {
        tokens
            .get(..end)?
            .iter()
            .rev()
            .find(|token| !matches!(token, SqlToken::Comment(_)))
    }

    fn cursor_has_existing_equals_before_qualified_identifier(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let Some(chain_start) =
            Self::current_qualified_identifier_chain_start(tokens, cursor_token_len)
        else {
            return false;
        };

        matches!(
            Self::previous_non_comment_token(tokens, chain_start),
            Some(SqlToken::Symbol(symbol)) if symbol == "="
        )
    }

    fn supports_qualified_condition_comparison_suggestions(
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        matches!(
            phase,
            intellisense_context::SqlPhase::JoinCondition
                | intellisense_context::SqlPhase::WhereClause
                | intellisense_context::SqlPhase::HavingClause
                | intellisense_context::SqlPhase::ConnectByClause
                | intellisense_context::SqlPhase::StartWithClause
                | intellisense_context::SqlPhase::MatchRecognizeClause
        )
    }

    fn current_query_tables_for_condition_completion(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<intellisense_context::ScopedTableRef> {
        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let current_query_tables = if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            intellisense_context::collect_tables_in_statement_declared_before_cursor(
                current_query_tokens,
                Self::cursor_token_len_in_current_query(deep_ctx),
            )
        } else {
            intellisense_context::collect_tables_in_statement(current_query_tokens)
        };
        if current_query_tables.is_empty() {
            deep_ctx.tables_in_scope.clone()
        } else {
            current_query_tables
        }
    }

    fn comparison_scope_tables_for_context(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<intellisense_context::ScopedTableRef> {
        let mut tables = Self::current_query_tables_for_condition_completion(deep_ctx);

        // In JOIN ON we deliberately exclude later-declared tables (see
        // `current_query_tables_for_condition_completion`). Skip the outer-scope
        // merge here so we don't re-introduce a table that lives in
        // `tables_in_scope` only because the full statement was parsed.
        if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            return tables;
        }

        for table in &deep_ctx.tables_in_scope {
            let already_present = tables.iter().any(|existing| {
                // `is_cte` is metadata, not identity: the FROM scope collector
                // flags every reference `is_cte = false` while `tables_in_scope`
                // flags CTE references `is_cte = true`. A relation is identified
                // by name + alias + depth only, so the flag is excluded here to
                // avoid double-listing a CTE reference.
                existing.depth == table.depth
                    && existing.name.eq_ignore_ascii_case(&table.name)
                    && match (&existing.alias, &table.alias) {
                        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
                        (None, None) => true,
                        _ => false,
                    }
            });
            if !already_present {
                tables.push(table.clone());
            }
        }

        tables
    }

    fn comparison_lookup_tables_for_context(
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let Some(qualifier) = qualifier else {
            return Vec::new();
        };
        if !Self::supports_qualified_condition_comparison_suggestions(deep_ctx.phase) {
            return Vec::new();
        }
        if Self::cursor_has_existing_equals_before_qualified_identifier(deep_ctx) {
            return Vec::new();
        }

        let comparison_tables = Self::comparison_scope_tables_for_context(deep_ctx);
        if comparison_tables.is_empty() {
            return Vec::new();
        }

        let mut lookup_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        for table_ref in comparison_tables {
            let table_lookup =
                Self::comparison_column_lookup_table_for_table_ref(&table_ref, deep_ctx);
            if lookup_tables
                .iter()
                .all(|existing| !existing.eq_ignore_ascii_case(&table_lookup))
            {
                lookup_tables.push(table_lookup);
            }
        }
        lookup_tables
    }

    fn comparison_column_lookup_table_for_table_ref(
        table_ref: &intellisense_context::ScopedTableRef,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> String {
        Self::column_lookup_table_for_table_ref(table_ref, deep_ctx)
    }

    fn collect_qualified_condition_comparison_suggestions(
        data: &IntellisenseData,
        prefix: &str,
        qualifier: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        if !Self::supports_qualified_condition_comparison_suggestions(deep_ctx.phase) {
            return Vec::new();
        }
        if Self::cursor_has_existing_equals_before_qualified_identifier(deep_ctx) {
            return Vec::new();
        }

        let comparison_tables = Self::comparison_scope_tables_for_context(deep_ctx);
        if comparison_tables.is_empty() {
            return Vec::new();
        }

        let target_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        if target_tables.is_empty() {
            return Vec::new();
        }

        let left_scope = Self::render_select_list_wildcard_scope(qualifier);
        if left_scope.is_empty() {
            return Vec::new();
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut target_columns = Vec::new();
        let mut seen_target_columns = HashSet::new();
        for table in &target_tables {
            for column in data.get_columns_for_table(table) {
                let upper = Self::completion_identifier_lookup_upper(&column);
                if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                    continue;
                }
                if seen_target_columns.insert(upper.clone()) {
                    target_columns.push((upper, column));
                }
            }
        }
        if target_columns.is_empty() {
            return Vec::new();
        }

        let pattern_variables = matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::MatchRecognizeClause
        )
        .then(|| intellisense_context::extract_match_recognize_pattern_variables(
            Self::current_query_tokens(deep_ctx),
        ))
        .filter(|variables| {
            variables
                .iter()
                .any(|variable| variable.eq_ignore_ascii_case(qualifier))
        });
        if let Some(pattern_variables) = pattern_variables {
            let mut suggestions = Vec::new();
            let mut seen_suggestions = HashSet::new();

            for other_pattern in pattern_variables {
                if Self::completion_identifiers_match(&other_pattern, qualifier) {
                    continue;
                }

                let rendered_other_scope =
                    Self::render_select_list_wildcard_scope(other_pattern.as_str());
                if rendered_other_scope.is_empty() {
                    continue;
                }

                for (_, target_column) in &target_columns {
                    let suggestion = format!(
                        "{}.{} = {}.{}",
                        left_scope,
                        Self::quote_identifier_segment_for_completion(target_column),
                        rendered_other_scope,
                        Self::quote_identifier_segment_for_completion(target_column),
                    );
                    if seen_suggestions.insert(suggestion.to_ascii_uppercase()) {
                        suggestions.push(suggestion);
                        if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                            return suggestions;
                        }
                    }
                }
            }

            return suggestions;
        }

        // In a JOIN ON clause the comparison is most naturally written against the
        // table being joined, i.e. the most recently declared one (e.g. `c` in
        // `JOIN ... c ON b.|`). Offer comparisons against later-declared tables first
        // so the current join target is suggested before earlier tables.
        let ordered_comparison_tables: Vec<&intellisense_context::ScopedTableRef> = if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            comparison_tables.iter().rev().collect()
        } else {
            comparison_tables.iter().collect()
        };

        let mut suggestions = Vec::new();
        let mut seen_suggestions = HashSet::new();
        for table_ref in ordered_comparison_tables {
            let other_scope_name = table_ref
                .alias
                .as_deref()
                .unwrap_or(table_ref.name.as_str());
            if Self::completion_identifiers_match(other_scope_name, qualifier) {
                continue;
            }

            let rendered_other_scope = Self::render_select_list_wildcard_scope(other_scope_name);
            if rendered_other_scope.is_empty() {
                continue;
            }

            let mut other_columns_by_upper = HashMap::new();
            let other_lookup_table =
                Self::comparison_column_lookup_table_for_table_ref(table_ref, deep_ctx);
            for column in data.get_columns_for_table(&other_lookup_table) {
                other_columns_by_upper
                    .entry(Self::completion_identifier_lookup_upper(&column))
                    .or_insert(column);
            }
            if other_columns_by_upper.is_empty() {
                continue;
            }

            for (upper, target_column) in &target_columns {
                let Some(other_column) = other_columns_by_upper.get(upper) else {
                    continue;
                };
                let suggestion = format!(
                    "{}.{} = {}.{}",
                    left_scope,
                    Self::quote_identifier_segment_for_completion(target_column),
                    rendered_other_scope,
                    Self::quote_identifier_segment_for_completion(other_column),
                );
                if seen_suggestions.insert(suggestion.to_ascii_uppercase()) {
                    suggestions.push(suggestion);
                    if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                        return suggestions;
                    }
                }
            }
        }

        suggestions
    }

    fn merge_suggestions_with_derived_columns(
        mut base: Vec<String>,
        prefix: &str,
        derived_columns: Vec<String>,
    ) -> Vec<String> {
        if derived_columns.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut seen: HashSet<String> = base
            .iter()
            .map(|item| Self::completion_identifier_lookup_upper(item))
            .collect();

        for column in derived_columns {
            let upper = Self::completion_identifier_lookup_upper(&column);
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                base.push(column);
            }
        }

        base.truncate(MAX_MERGED_SUGGESTIONS);
        base
    }

    fn merge_suggestions_with_prioritized_derived_columns(
        base: Vec<String>,
        prefix: &str,
        derived_columns: Vec<String>,
    ) -> Vec<String> {
        if derived_columns.is_empty() {
            let mut base = base;
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut seen = HashSet::new();
        let mut merged = Vec::new();

        for column in derived_columns {
            let upper = Self::completion_identifier_lookup_upper(&column);
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                merged.push(column);
                if merged.len() >= MAX_MERGED_SUGGESTIONS {
                    return merged;
                }
            }
        }

        for item in base {
            let upper = Self::completion_identifier_lookup_upper(&item);
            if seen.insert(upper) {
                merged.push(item);
                if merged.len() >= MAX_MERGED_SUGGESTIONS {
                    break;
                }
            }
        }

        merged
    }

    fn collect_derived_columns_for_context(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let mut derived_columns =
            intellisense_context::extract_oracle_unpivot_generated_columns(current_query_tokens);
        derived_columns.extend(
            intellisense_context::extract_oracle_model_generated_columns(current_query_tokens),
        );
        derived_columns.extend(
            intellisense_context::extract_match_recognize_generated_columns(current_query_tokens),
        );

        if Self::cursor_is_in_query_level_order_by(deep_ctx) {
            derived_columns.extend(intellisense_context::extract_select_list_columns(
                current_query_tokens,
            ));
        }

        Self::dedup_column_names_case_insensitive(&mut derived_columns);
        derived_columns
    }

    fn maybe_prefetch_columns_for_word(
        word: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        if word.is_empty() {
            return;
        }

        let should_prefetch = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::resolve_table_column_load_key(&data, word).is_some()
        };

        if should_prefetch {
            Self::request_table_columns(word, intellisense_data, column_sender, connection);
        }
    }

    fn request_table_columns(
        table_name: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        let table_key = {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(selected) = Self::resolve_table_column_load_key(&data, table_name) else {
                return;
            };
            if !data.mark_columns_loading(&selected) {
                return;
            }
            selected
        };

        let task = ColumnLoadTask {
            table_key,
            connection: connection.clone(),
            sender: column_sender.clone(),
            foreign_keys: false,
        };

        if let Err(task) = Self::enqueue_column_load_task(task) {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                &format!(
                    "failed to enqueue column loader task for {}",
                    task.table_key
                ),
            );
            let _ = task.sender.send(ColumnLoadUpdate {
                table: task.table_key,
                columns: Vec::new(),
                column_meta: HashMap::new(),
                foreign_keys: Vec::new(),
                is_foreign_keys: false,
                cache_columns: false,
            });
            app::awake();
        }
    }

    /// Enqueue a lazy foreign-key load for `table_name` (used only when filling
    /// a JOIN ... ON clause), deduplicated against in-flight/loaded keys.
    fn request_table_foreign_keys(
        table_name: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        let table_key = {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(selected) = Self::resolve_table_column_load_key(&data, table_name) else {
                return;
            };
            if !data.mark_foreign_keys_loading(&selected) {
                return;
            }
            selected
        };

        let task = ColumnLoadTask {
            table_key,
            connection: connection.clone(),
            sender: column_sender.clone(),
            foreign_keys: true,
        };

        if let Err(task) = Self::enqueue_column_load_task(task) {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                &format!(
                    "failed to enqueue foreign-key loader task for {}",
                    task.table_key
                ),
            );
            let _ = task.sender.send(ColumnLoadUpdate {
                table: task.table_key,
                columns: Vec::new(),
                column_meta: HashMap::new(),
                foreign_keys: Vec::new(),
                is_foreign_keys: true,
                cache_columns: false,
            });
            app::awake();
        }
    }

    fn resolve_table_column_load_key(
        data: &IntellisenseData,
        table_name: &str,
    ) -> Option<String> {
        let candidates = Self::table_lookup_key_candidates(table_name);
        let normalized = candidates.first()?.trim();
        if normalized.is_empty() {
            return None;
        }

        let has_unquoted_dot = Self::has_unquoted_dot(table_name);
        if has_unquoted_dot {
            let segments = Self::relation_name_segments(table_name)?;
            if segments.len() >= 2 {
                let qualifier = segments[..segments.len() - 1].join(".");
                let member = segments.last()?;
                if data.qualifier_has_member(&qualifier, member, true)
                    || data.qualifier_has_member(&qualifier, member, false)
                {
                    return Some(normalized.to_ascii_uppercase());
                }
            }
        }

        if !has_unquoted_dot && data.is_known_relation(normalized) {
            return Some(normalized.to_ascii_uppercase());
        }

        if !normalized.contains('.') {
            if let Some(default_qualifier) = data.default_qualifier() {
                if data.qualifier_has_member(default_qualifier, normalized, true)
                    || data.qualifier_has_member(default_qualifier, normalized, false)
                {
                    return Some(
                        format!("{}.{}", default_qualifier, normalized).to_ascii_uppercase(),
                    );
                }
            }
        }

        if data.is_known_relation(normalized) {
            return Some(normalized.to_ascii_uppercase());
        }

        candidates
            .iter()
            .skip(1)
            .find(|candidate| data.is_known_relation(candidate))
            .map(|candidate| candidate.to_ascii_uppercase())
    }

    fn table_lookup_key_candidates(table_name: &str) -> Vec<String> {
        let Some(segments) = Self::relation_name_segments(table_name) else {
            return Vec::new();
        };
        let normalized = segments.join(".");
        if normalized.is_empty() {
            return Vec::new();
        }

        let mut candidates = vec![normalized.clone()];
        if Self::has_unquoted_dot(table_name) {
            if let Some(last) = segments.last() {
                if !last.eq_ignore_ascii_case(&normalized) && !last.trim().is_empty() {
                    candidates.push(last.trim().to_string());
                }
            }
        }

        candidates
    }

    fn relation_name_segments(value: &str) -> Option<Vec<String>> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;

        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
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
                    let segment = Self::strip_identifier_quotes(current.trim());
                    if !segment.is_empty() {
                        parts.push(segment);
                    } else {
                        return None;
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        if active_quote.is_some() {
            return None;
        }

        let segment = Self::strip_identifier_quotes(current.trim());
        if !segment.is_empty() {
            parts.push(segment);
        } else {
            return None;
        }

        Some(parts)
    }

    fn has_unquoted_dot(value: &str) -> bool {
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;
        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
                    if active_quote == Some(ch) {
                        if chars.peek().copied() == Some(ch) {
                            chars.next();
                        } else {
                            active_quote = None;
                        }
                    } else if active_quote.is_none() {
                        active_quote = Some(ch);
                    }
                }
                '[' if active_quote.is_none() => active_quote = Some(']'),
                ']' if active_quote == Some(']') => {
                    if chars.peek().copied() == Some(']') {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                '.' if active_quote.is_none() => return true,
                _ => {}
            }
        }
        false
    }

    fn render_select_list_wildcard_scope(scope_name: &str) -> String {
        let segments = Self::relation_name_segments(scope_name).unwrap_or_else(|| {
            let stripped = Self::strip_identifier_quotes(scope_name);
            if stripped.trim().is_empty() {
                Vec::new()
            } else {
                vec![stripped]
            }
        });
        if segments.is_empty() {
            return String::new();
        }

        segments
            .into_iter()
            .map(|segment| Self::quote_identifier_segment_for_completion(&segment))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn quote_identifier_segment_for_completion(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "\"\"".to_string();
        }
        if sql_text::is_quoted_identifier(trimmed) {
            return trimmed.to_string();
        }
        if Self::is_unquoted_completion_identifier(trimmed) {
            return trimmed.to_string();
        }

        format!("\"{}\"", trimmed.replace('"', "\"\""))
    }

    fn completion_identifier_lookup_upper(text: &str) -> String {
        let trimmed = text.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            return sql_text::strip_identifier_quotes(trimmed).to_ascii_uppercase();
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            return trimmed[1..trimmed.len().saturating_sub(1)]
                .replace("]]", "]")
                .to_ascii_uppercase();
        }

        match trimmed.chars().next() {
            Some('"') | Some('`') | Some('[') => trimmed[1..].to_ascii_uppercase(),
            _ => trimmed.to_ascii_uppercase(),
        }
    }

    fn completion_wildcard_candidate_lookup_upper(text: &str) -> String {
        let trimmed = text.trim();
        let lookup_text = trimmed.strip_suffix(".*").unwrap_or(trimmed);
        Self::completion_identifier_lookup_upper(lookup_text)
    }

    fn completion_identifiers_match(left: &str, right: &str) -> bool {
        Self::completion_identifier_lookup_upper(left) == Self::completion_identifier_lookup_upper(right)
    }

    fn is_unquoted_completion_identifier(text: &str) -> bool {
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '#')) {
            return false;
        }

        chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    }
}
