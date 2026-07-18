use super::formatter::{FormatManagedFrameKind, ListOwnerKind};
use super::{query_text, SqlEditorWidget, SqlToken};
use crate::db::connection::DatabaseType;
use crate::db::{QueryExecutor, ScriptItem, ToolCommand};
use crate::sql_text;
use std::any::Any;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

const FORMAT_SWEEP_DETAIL_LIMIT: usize = 200;
const FORMAT_SWEEP_INDENT_WIDTH: usize = 4;
const FORMAT_SWEEP_FRAME_REGRESSION_CASES: &[(DatabaseType, &str)] = &[
    (
        DatabaseType::Oracle,
        "SELECT CASE WHEN (SELECT COUNT(*) FROM orders) = 6 AND (SELECT COUNT(*) FROM orders) = 7 THEN 1 ELSE 0 END AS flag FROM t LEFT JOIN u ON /* join child */ u.id = t.id;",
    ),
    (
        DatabaseType::MySQL,
        "SELECT CASE WHEN (SELECT COUNT(*) FROM orders) = 6 AND (SELECT COUNT(*) FROM orders) = 7 THEN 1 ELSE 0 END AS flag FROM t LEFT JOIN u ON /* join child */ u.id = t.id;",
    ),
    (
        DatabaseType::MariaDB,
        "SELECT CASE WHEN (SELECT COUNT(*) FROM orders) = 6 AND (SELECT COUNT(*) FROM orders) = 7 THEN 1 ELSE 0 END AS flag FROM t LEFT JOIN u ON /* join child */ u.id = t.id;",
    ),
];
const FORMAT_SWEEP_STRUCTURAL_REGRESSION_CASES: &[(DatabaseType, &str)] = &[
    (
        DatabaseType::Oracle,
        "WITH x AS (SELECT LAG(v) OVER (PARTITION BY g ORDER BY id) AS previous_v, v FROM t) SELECT previous_v FROM x;",
    ),
    (
        DatabaseType::Oracle,
        "MERGE INTO dst d USING (SELECT s.id, u.v FROM src s JOIN aux u ON u.id = s.id) x ON (d.id = x.id AND d.v <> x.v) WHEN MATCHED THEN UPDATE SET d.v = x.v WHEN NOT MATCHED THEN INSERT (id, v) VALUES (x.id, x.v);",
    ),
    (
        DatabaseType::Oracle,
        "BEGIN EXECUTE IMMEDIATE 'BEGIN p(:1,:2,:3); END;' USING 1, CASE WHEN a = 1 AND b = 2 THEN 2 ELSE 3 END, 4; END;",
    ),
    (
        DatabaseType::MySQL,
        "CREATE PROCEDURE p() BEGIN CALL assert_fn(a = 1 AND b = 2, 'x'); END;",
    ),
    (
        DatabaseType::MariaDB,
        "UPDATE t JOIN u ON u.id = t.id SET t.v = CASE WHEN IFNULL(u.v, 0) > 0 AND u.flag = 'Y' THEN u.v ELSE 0 END WHERE t.active = 1 AND u.active = 1;",
    ),
    (
        DatabaseType::Oracle,
        "MERGE INTO t trg USING src ON (trg.id = src.id) WHEN MATCHED THEN UPDATE SET trg.a = src.a -- comment\n, trg.b = src.b;",
    ),
    (
        DatabaseType::Oracle,
        "WITH a (n) AS (SELECT 1 FROM DUAL UNION ALL SELECT n + 1 FROM a WHERE n < 3) SEARCH DEPTH FIRST BY n SET ord CYCLE n SET cyc TO 'Y' DEFAULT 'N', b AS (SELECT n FROM a) SELECT n FROM b;",
    ),
    (
        DatabaseType::Oracle,
        "BEGIN v := (SELECT COUNT(*) FROM t WHERE t.id = 1); END;",
    ),
    (
        DatabaseType::Oracle,
        "UPDATE t SET a = (SELECT MAX(x) FROM s WHERE s.id = t.id), b = (SELECT MIN(x) FROM s WHERE s.id = t.id) WHERE t.id = 1;",
    ),
    (
        DatabaseType::MariaDB,
        "CREATE FUNCTION f(a DECIMAL(12, 2), b DECIMAL(14, 2)) RETURNS DECIMAL(18, 2) DETERMINISTIC BEGIN RETURN a + b; END;",
    ),
];
const FORMAT_SWEEP_INLINE_QUERY_FRAME_REGRESSION: &str =
    "SELECT JSON_OBJECT ('grand' VALUE (SELECT SUM(amount) FROM orders) RETURNING CLOB) AS payload FROM DUAL;";
const FORMAT_SWEEP_INLINE_SELECT_COMMENT_REGRESSION: &str =
    "WITH x AS (SELECT /* first child */ a, b FROM t) SELECT a, b FROM x;";
const FORMAT_SWEEP_WITH_FRAME_REGRESSION: &str =
    "WITH first_cte AS (SELECT 1 AS id), second_cte AS (SELECT id FROM first_cte), third_cte AS (SELECT id FROM second_cte) SELECT id FROM third_cte;";
const FORMAT_SWEEP_INLINE_CONTINUATION_REGRESSIONS: &[&str] = &[
    "SELECT first_value + -- continuation\nsecond_value AS total, third_value FROM sample_table;",
    "SELECT * FROM sample_table WHERE first_value = -- continuation\nsecond_value AND third_value = 3;",
    "SELECT calculate(first_value + -- continuation\nsecond_value, third_value) AS total FROM sample_table;",
    "WITH only_cte AS (SELECT first_value + -- continuation\nsecond_value AS total FROM sample_table) SELECT total FROM only_cte;",
];
const FORMAT_SWEEP_EXECUTABLE_BOUNDARY_REGRESSION_CASES: &[(DatabaseType, &str)] = &[
    (
        DatabaseType::MySQL,
        "DESCRIBE t;\nEXPLAIN SELECT * FROM t;\nSHOW PROCESSLIST;\nDROP USER IF EXISTS 'u'@'localhost';\nSET @value = 1;\nSELECT @value;",
    ),
    (
        DatabaseType::MariaDB,
        "SET autocommit = 0;\nINSERT INTO t VALUES (1);\nSHOW WARNINGS;\nSELECT 1;",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FormatSweepIssueKind {
    FormatPanic,
    ItemOrTokenChanged,
    ExecutableBoundary,
    IdentifierCase,
    Indentation,
    FrameAlignment,
    LineBreak,
    WhitespaceDependency,
    NonIdempotent,
}

#[derive(Clone, Debug)]
struct FormatSweepIssue {
    kind: FormatSweepIssueKind,
    line: usize,
    column: usize,
    marker_offset: usize,
    message: String,
}

#[derive(Debug)]
struct FormatSweepRun {
    formatted: String,
    checked_lines: usize,
    checked_gaps: usize,
    checked_identifier_case_words: usize,
    checked_frames: usize,
    checked_frame_boundaries: usize,
    checked_frame_depth_symmetries: usize,
    checked_frame_body_items: usize,
    checked_frame_closes: usize,
    managed_frame_kinds: Vec<FormatManagedFrameKind>,
    managed_list_owner_kinds: Vec<ListOwnerKind>,
    probes: usize,
    issues: Vec<FormatSweepIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FormatSweepToken {
    Word(String),
    String(String),
    Comment(String),
    Symbol(String),
}

#[derive(Clone, Copy, Debug)]
enum FormatSweepProbeKind {
    Reindent,
    CollapseBreaks,
    ExpandInline,
}

impl FormatSweepIssue {
    fn new(kind: FormatSweepIssueKind, text: &str, offset: usize, message: String) -> Self {
        let offset = clamp_char_boundary(text, offset);
        let (line, column) = line_column(text, offset);
        Self {
            kind,
            line,
            column,
            marker_offset: offset,
            message,
        }
    }
}

fn clamp_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = clamp_char_boundary(text, offset);
    let before = text.get(..offset).unwrap_or_default();
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |idx| idx + 1);
    let column = text
        .get(line_start..offset)
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line, column)
}

fn leading_spaces(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn mysql_compatible(db_type: DatabaseType) -> bool {
    matches!(db_type, DatabaseType::MySQL | DatabaseType::MariaDB)
}

#[derive(Clone, Debug)]
struct FormatSweepScriptTokenSpan {
    statement_index: usize,
    token: SqlToken,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct FormatSweepStatementSpan {
    start: usize,
    end: usize,
    token_start: usize,
    token_end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FormatSweepGapMutationPolicy {
    All,
    PreserveLineBreak,
    Preserve,
}

impl FormatSweepGapMutationPolicy {
    fn allows(self, kind: FormatSweepProbeKind) -> bool {
        match self {
            Self::All => true,
            Self::PreserveLineBreak => !matches!(kind, FormatSweepProbeKind::CollapseBreaks),
            Self::Preserve => false,
        }
    }
}

struct FormatSweepDocument<'a> {
    text: &'a str,
    db_type: DatabaseType,
    statements: Vec<FormatSweepStatementSpan>,
    tokens: Vec<FormatSweepScriptTokenSpan>,
}

impl<'a> FormatSweepDocument<'a> {
    fn new(text: &'a str, db_type: DatabaseType) -> Self {
        let mut tokens = Vec::new();
        let statements = QueryExecutor::statement_spans_for_db_type_with_mysql_delimiter(
            text,
            Some(db_type),
            None,
        )
        .into_iter()
        .enumerate()
        .filter_map(|(statement_index, (start, end))| {
            let statement = text.get(start..end)?;
            let token_start = tokens.len();
            tokens.extend(
                query_text::tokenize_sql_spanned_with_mysql_compat(
                    statement,
                    mysql_compatible(db_type),
                )
                .into_iter()
                .map(|span| FormatSweepScriptTokenSpan {
                    statement_index,
                    token: span.token,
                    start: start.saturating_add(span.start),
                    end: start.saturating_add(span.end),
                }),
            );
            Some(FormatSweepStatementSpan {
                start,
                end,
                token_start,
                token_end: tokens.len(),
            })
        })
        .collect();
        Self {
            text,
            db_type,
            statements,
            tokens,
        }
    }

    fn statement_fingerprint(&self) -> Vec<Vec<FormatSweepToken>> {
        QueryExecutor::split_script_items_for_db_type(self.text, Some(self.db_type))
            .into_iter()
            .filter_map(|item| match item {
                ScriptItem::Statement(statement) => {
                    let tokens = query_text::tokenize_sql_spanned_with_mysql_compat(
                        &statement,
                        mysql_compatible(self.db_type),
                    )
                    .into_iter()
                    .map(|span| match span.token {
                        SqlToken::Word(word) => FormatSweepToken::Word(word.to_ascii_uppercase()),
                        SqlToken::String(value) => FormatSweepToken::String(value),
                        SqlToken::Comment(value) => {
                            FormatSweepToken::Comment(value.trim_end().to_string())
                        }
                        SqlToken::Symbol(value) => FormatSweepToken::Symbol(value),
                    })
                    .collect::<Vec<_>>();
                    let is_tool_option_continuation = matches!(
                        tokens.as_slice(),
                        [FormatSweepToken::Word(on)] if on == "ON" || on == "OFF"
                    ) || matches!(
                        tokens.as_slice(),
                        [
                            FormatSweepToken::Word(on),
                            FormatSweepToken::Word(size),
                            FormatSweepToken::Word(unlimited)
                        ] if on == "ON" && size == "SIZE" && unlimited == "UNLIMITED"
                    );
                    (!is_tool_option_continuation).then_some(tokens)
                }
                ScriptItem::ToolCommand(_) => None,
            })
            .collect()
    }

    fn protected_payload_lines(&self, line_count: usize) -> Vec<bool> {
        let mut protected = vec![false; line_count];
        let line_starts = line_start_offsets(self.text);
        for span in &self.tokens {
            if !token_is_comment_or_string(&span.token)
                || !self
                    .text
                    .get(span.start..span.end)
                    .is_some_and(|token| token.contains('\n'))
            {
                continue;
            }
            let start_line = line_starts
                .partition_point(|line_start| *line_start <= span.start)
                .saturating_sub(1);
            let end_line = line_starts
                .partition_point(|line_start| *line_start < span.end)
                .saturating_sub(1);
            for line in start_line..=end_line.min(protected.len().saturating_sub(1)) {
                protected[line] = true;
            }
        }
        protected
    }

    fn safe_gap_count(&self) -> usize {
        self.tokens
            .windows(2)
            .filter(|pair| {
                let [left, right] = pair else {
                    return false;
                };
                left.statement_index == right.statement_index
                    && !token_is_comment_or_string(&left.token)
                    && !token_is_comment_or_string(&right.token)
                    && self.text.get(left.end..right.start).is_some_and(|gap| {
                        !gap.is_empty() && gap.bytes().all(|byte| byte.is_ascii_whitespace())
                    })
            })
            .count()
    }

    fn gap_mutation_policy(
        &self,
        previous: Option<&FormatSweepScriptTokenSpan>,
        current: &FormatSweepScriptTokenSpan,
        gap: &str,
    ) -> FormatSweepGapMutationPolicy {
        if gap.is_empty()
            || !gap.bytes().all(|byte| byte.is_ascii_whitespace())
            || previous.is_some_and(|span| token_is_comment_or_string(&span.token))
            || token_is_comment_or_string(&current.token)
        {
            return FormatSweepGapMutationPolicy::Preserve;
        }

        let split_line = self
            .text
            .get(current.start..)
            .unwrap_or_default()
            .split('\n')
            .next()
            .unwrap_or_default()
            .trim();
        let would_start_script_command = query_text::is_sqlplus_command_line(split_line);
        let would_leave_script_command = previous.is_some_and(|previous| {
            let line_start = self
                .text
                .get(..previous.end)
                .and_then(|prefix| prefix.rfind('\n'))
                .map_or(0, |idx| idx.saturating_add(1));
            self.text
                .get(line_start..previous.end)
                .is_some_and(|line| query_text::is_sqlplus_command_line(line.trim()))
        });
        let line_break_is_safe_to_collapse = !gap.contains('\n')
            || previous.is_some_and(|span| token_keeps_following_gap_inline(&span.token))
            || token_keeps_preceding_gap_inline(&current.token);
        let preserves_named_end_suffix = matches!(
            (previous.map(|span| &span.token), &current.token),
            (Some(SqlToken::Word(previous_word)), SqlToken::Word(word))
                if previous_word.eq_ignore_ascii_case("END")
                    && !sql_text::is_format_block_end_qualifier_keyword(word)
        );
        if would_start_script_command || would_leave_script_command || preserves_named_end_suffix {
            FormatSweepGapMutationPolicy::Preserve
        } else if !line_break_is_safe_to_collapse {
            FormatSweepGapMutationPolicy::PreserveLineBreak
        } else {
            FormatSweepGapMutationPolicy::All
        }
    }

    fn render_probe(&self, kind: FormatSweepProbeKind) -> String {
        let mut probe = String::with_capacity(self.text.len().saturating_add(128));
        let mut document_cursor = 0usize;
        for statement in &self.statements {
            probe.push_str(
                self.text
                    .get(document_cursor..statement.start)
                    .unwrap_or_default(),
            );
            let spans = &self.tokens[statement.token_start..statement.token_end];
            let mut statement_cursor = statement.start;
            for (idx, span) in spans.iter().enumerate() {
                let gap = self
                    .text
                    .get(statement_cursor..span.start)
                    .unwrap_or_default();
                if self
                    .gap_mutation_policy(idx.checked_sub(1).and_then(|i| spans.get(i)), span, gap)
                    .allows(kind)
                {
                    match kind {
                        FormatSweepProbeKind::Reindent if gap.contains('\n') => {
                            for _ in 0..gap.bytes().filter(|byte| *byte == b'\n').count() {
                                probe.push('\n');
                            }
                            probe.push_str("       ");
                        }
                        FormatSweepProbeKind::CollapseBreaks if gap.contains('\n') => {
                            probe.push(' ');
                        }
                        FormatSweepProbeKind::ExpandInline
                            if !gap.contains('\n') && idx % 3 == 0 =>
                        {
                            probe.push_str("\n       ");
                        }
                        _ => probe.push_str(gap),
                    }
                } else {
                    probe.push_str(gap);
                }
                probe.push_str(self.text.get(span.start..span.end).unwrap_or_default());
                statement_cursor = span.end;
            }
            probe.push_str(
                self.text
                    .get(statement_cursor..statement.end)
                    .unwrap_or_default(),
            );
            document_cursor = statement.end;
        }
        probe.push_str(self.text.get(document_cursor..).unwrap_or_default());
        probe
    }
}

fn token_keeps_following_gap_inline(token: &SqlToken) -> bool {
    match token {
        SqlToken::Word(word) => sql_text::is_format_expression_continuation_keyword(word),
        SqlToken::Symbol(symbol) => matches!(
            symbol.as_str(),
            "(" | ","
                | ":="
                | "=>"
                | "="
                | "<"
                | ">"
                | "<="
                | ">="
                | "<>"
                | "!="
                | "<=>"
                | "+"
                | "-"
                | "*"
                | "/"
                | "%"
                | "||"
                | "|"
                | "^"
        ),
        SqlToken::String(_) | SqlToken::Comment(_) => false,
    }
}

fn token_keeps_preceding_gap_inline(token: &SqlToken) -> bool {
    match token {
        SqlToken::Word(word) => sql_text::is_format_expression_continuation_keyword(word),
        SqlToken::Symbol(symbol) => matches!(symbol.as_str(), ")" | ","),
        SqlToken::String(_) | SqlToken::Comment(_) => false,
    }
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "panic".to_string()
}

fn line_is_auditable_code(
    line: &str,
    in_block_comment: &mut bool,
    protected_payload: bool,
) -> bool {
    if line.trim().is_empty()
        || line.trim() == "/"
        || protected_payload
        || sql_text::line_is_comment_only_with_block_state(line, in_block_comment)
        || QueryExecutor::parse_tool_command_if_candidate(line).is_some()
    {
        return false;
    }
    true
}

fn format_sweep_audit_first_pass(
    formatted: &str,
    db_type: DatabaseType,
) -> (usize, usize, Vec<FormatSweepIssue>) {
    let mut issues = Vec::new();
    let lines: Vec<&str> = formatted.lines().collect();
    let line_starts = line_start_offsets(formatted);
    let document = FormatSweepDocument::new(formatted, db_type);
    let protected_lines = document.protected_payload_lines(lines.len());
    let mut in_block_comment = false;
    let mut checked_lines = 0usize;

    for (idx, line) in lines.iter().enumerate() {
        if !line_is_auditable_code(
            line,
            &mut in_block_comment,
            protected_lines.get(idx).copied().unwrap_or(false),
        ) {
            continue;
        }
        checked_lines += 1;
        let leading_len = line.len().saturating_sub(line.trim_start().len());
        let leading = line.get(..leading_len).unwrap_or_default();
        let offset = line_starts.get(idx).copied().unwrap_or(0);
        if line.ends_with([' ', '\t']) {
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::Indentation,
                formatted,
                offset.saturating_add(line.trim_end_matches([' ', '\t']).len()),
                "code line has trailing whitespace".to_string(),
            ));
        }
        if leading.bytes().any(|byte| byte == b'\t') {
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::Indentation,
                formatted,
                offset,
                "code line starts with a tab; formatter indentation must use spaces".to_string(),
            ));
            continue;
        }
        let actual_spaces = leading.bytes().filter(|byte| *byte == b' ').count();
        if actual_spaces % FORMAT_SWEEP_INDENT_WIDTH != 0 {
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::Indentation,
                formatted,
                offset,
                format!(
                    "code line has {actual_spaces} leading spaces; expected a multiple of {FORMAT_SWEEP_INDENT_WIDTH}"
                ),
            ));
        }
    }

    (checked_lines, document.safe_gap_count(), issues)
}

fn format_sweep_audit_token_count(
    source: &str,
    formatted: &str,
    db_type: DatabaseType,
) -> Option<FormatSweepIssue> {
    let source_fingerprint = FormatSweepDocument::new(source, db_type).statement_fingerprint();
    let formatted_fingerprint =
        FormatSweepDocument::new(formatted, db_type).statement_fingerprint();
    (source_fingerprint != formatted_fingerprint).then(|| {
        let first_mismatch = source_fingerprint
            .iter()
            .zip(formatted_fingerprint.iter())
            .position(|(left, right)| left != right)
            .unwrap_or_else(|| source_fingerprint.len().min(formatted_fingerprint.len()));
        let first_token_mismatch = source_fingerprint
            .get(first_mismatch)
            .zip(formatted_fingerprint.get(first_mismatch))
            .map(|(source_tokens, formatted_tokens)| {
                source_tokens
                    .iter()
                    .zip(formatted_tokens.iter())
                    .position(|(left, right)| left != right)
                    .unwrap_or_else(|| source_tokens.len().min(formatted_tokens.len()))
            });
        let preview_start = first_token_mismatch.unwrap_or(0).saturating_sub(3);
        let source_preview = source_fingerprint
            .get(first_mismatch)
            .map(|tokens| {
                tokens
                    .iter()
                    .skip(preview_start)
                    .take(7)
                    .cloned()
                    .collect::<Vec<_>>()
            });
        let formatted_preview = formatted_fingerprint
            .get(first_mismatch)
            .map(|tokens| {
                tokens
                    .iter()
                    .skip(preview_start)
                    .take(7)
                    .cloned()
                    .collect::<Vec<_>>()
            });
        FormatSweepIssue::new(
            FormatSweepIssueKind::ItemOrTokenChanged,
            formatted,
            0,
            format!(
                "first formatting pass changed SQL statement items or tokens at item {}, token {:?}; item counts {} -> {}; source={source_preview:?} formatted={formatted_preview:?}",
                first_mismatch + 1,
                first_token_mismatch.map(|idx| idx + 1),
                source_fingerprint.len(),
                formatted_fingerprint.len()
            ),
        )
    })
}

fn format_sweep_audit_executable_boundaries(
    formatted: &str,
    db_type: DatabaseType,
) -> Vec<FormatSweepIssue> {
    if !mysql_compatible(db_type) {
        return Vec::new();
    }

    let line_starts = line_start_offsets(formatted);
    let mut issues: Vec<FormatSweepIssue> = formatted
        .lines()
        .enumerate()
        .filter_map(|(line_idx, line)| {
            let command = QueryExecutor::parse_tool_command_if_candidate(line.trim())?;
            let is_server_statement = matches!(
                command,
                ToolCommand::Use { .. }
                    | ToolCommand::Describe { .. }
                    | ToolCommand::SetAutoCommit { .. }
                    | ToolCommand::ShowDatabases
                    | ToolCommand::ShowTables
                    | ToolCommand::ShowColumns { .. }
                    | ToolCommand::ShowCreateTable { .. }
                    | ToolCommand::ShowProcessList
                    | ToolCommand::ShowVariables { .. }
                    | ToolCommand::ShowStatus { .. }
                    | ToolCommand::ShowWarnings
                    | ToolCommand::MysqlShowErrors
            );
            (is_server_statement && !sql_text::line_ends_with_semicolon_before_inline_comment(line))
                .then(|| {
                    FormatSweepIssue::new(
                        FormatSweepIssueKind::ExecutableBoundary,
                        formatted,
                        line_starts.get(line_idx).copied().unwrap_or_default(),
                        "MySQL-family server command lost its statement terminator".to_string(),
                    )
                })
        })
        .collect();

    let document = FormatSweepDocument::new(formatted, db_type);
    for tokens in document.tokens.windows(3) {
        let [left, separator, right] = tokens else {
            continue;
        };
        let is_account_component =
            |token: &SqlToken| matches!(token, SqlToken::Word(_) | SqlToken::String(_));
        if left.statement_index != separator.statement_index
            || separator.statement_index != right.statement_index
            || !matches!(&separator.token, SqlToken::Symbol(symbol) if symbol == "@")
            || !is_account_component(&left.token)
            || !is_account_component(&right.token)
            || !matches!(&left.token, SqlToken::String(_))
                && !matches!(&right.token, SqlToken::String(_))
        {
            continue;
        }
        let has_whitespace = formatted
            .get(left.end..separator.start)
            .is_some_and(|gap| !gap.is_empty())
            || formatted
                .get(separator.end..right.start)
                .is_some_and(|gap| !gap.is_empty());
        if has_whitespace {
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::ExecutableBoundary,
                formatted,
                separator.start,
                "MySQL-family account separator gained whitespace".to_string(),
            ));
        }
    }
    issues
}

fn token_is_comment_or_string(token: &SqlToken) -> bool {
    matches!(token, SqlToken::Comment(_) | SqlToken::String(_))
}

/// Audits that a formatting pass never changes the letter case of a token that
/// sits in an unambiguous identifier slot: a segment adjacent to a `.`
/// qualifier, a `GOTO` label target, or a `PROCEDURE`/`FUNCTION` object name.
/// Keyword-case normalization must not rewrite identifiers.
fn format_sweep_audit_identifier_case(
    source: &str,
    formatted: &str,
    db_type: DatabaseType,
) -> (usize, Vec<FormatSweepIssue>) {
    let source_document = FormatSweepDocument::new(source, db_type);
    let formatted_document = FormatSweepDocument::new(formatted, db_type);
    let source_words: Vec<&str> = source_document
        .tokens
        .iter()
        .filter_map(|span| match &span.token {
            SqlToken::Word(word) => Some(word.as_str()),
            _ => None,
        })
        .collect();
    let formatted_word_indices: Vec<usize> = formatted_document
        .tokens
        .iter()
        .enumerate()
        .filter_map(|(idx, span)| matches!(span.token, SqlToken::Word(_)).then_some(idx))
        .collect();
    if source_words.len() != formatted_word_indices.len() {
        // Token-count changes are reported by `format_sweep_audit_token_count`.
        return (0, Vec::new());
    }

    let mut issues = Vec::new();
    for (source_word, token_idx) in source_words
        .iter()
        .zip(formatted_word_indices.iter().copied())
    {
        let span = &formatted_document.tokens[token_idx];
        let SqlToken::Word(formatted_word) = &span.token else {
            continue;
        };
        if *source_word == formatted_word.as_str()
            || !source_word.eq_ignore_ascii_case(formatted_word)
        {
            continue;
        }
        let previous = formatted_document.tokens[..token_idx]
            .iter()
            .rev()
            .take_while(|prev_span| prev_span.statement_index == span.statement_index)
            .find(|prev_span| !matches!(prev_span.token, SqlToken::Comment(_)));
        let next = formatted_document
            .tokens
            .get(token_idx.saturating_add(1)..)
            .unwrap_or_default()
            .iter()
            .take_while(|next_span| next_span.statement_index == span.statement_index)
            .find(|next_span| !matches!(next_span.token, SqlToken::Comment(_)));
        let previous_is_dot = matches!(
            previous.map(|prev_span| &prev_span.token),
            Some(SqlToken::Symbol(sym)) if sym == "."
        );
        let next_is_dot = matches!(
            next.map(|next_span| &next_span.token),
            Some(SqlToken::Symbol(sym)) if sym == "."
        );
        let previous_names_object = matches!(
            previous.map(|prev_span| &prev_span.token),
            Some(SqlToken::Word(word))
                if word.eq_ignore_ascii_case("GOTO")
                    || word.eq_ignore_ascii_case("PROCEDURE")
                    || word.eq_ignore_ascii_case("FUNCTION")
        );
        if previous_is_dot || next_is_dot || previous_names_object {
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::IdentifierCase,
                formatted,
                span.start,
                format!(
                    "identifier `{source_word}` was case-changed to `{formatted_word}` in an identifier slot (dot-qualified or named-object position)"
                ),
            ));
        }
    }
    (source_words.len(), issues)
}

fn token_label(token: &SqlToken) -> &str {
    match token {
        SqlToken::Word(word)
        | SqlToken::String(word)
        | SqlToken::Comment(word)
        | SqlToken::Symbol(word) => word,
    }
}

fn whitespace_kind_between_equal_tokens(
    baseline: &str,
    comparison: &str,
    db_type: DatabaseType,
) -> (FormatSweepIssueKind, usize, String) {
    let left_document = FormatSweepDocument::new(baseline, db_type);
    let right_document = FormatSweepDocument::new(comparison, db_type);
    let left = &left_document.tokens;
    let right = &right_document.tokens;
    if left.len() != right.len() {
        return (
            FormatSweepIssueKind::ItemOrTokenChanged,
            0,
            "formatted outputs contain different token counts".to_string(),
        );
    }
    let mut left_cursor = 0usize;
    let mut right_cursor = 0usize;
    for (left_span, right_span) in left.iter().zip(right.iter()) {
        let left_gap = baseline
            .get(left_cursor..left_span.start)
            .unwrap_or_default();
        let right_gap = comparison
            .get(right_cursor..right_span.start)
            .unwrap_or_default();
        if left_gap != right_gap {
            let left_newlines = left_gap.bytes().filter(|byte| *byte == b'\n').count();
            let right_newlines = right_gap.bytes().filter(|byte| *byte == b'\n').count();
            let kind = if left_newlines != right_newlines {
                FormatSweepIssueKind::LineBreak
            } else {
                FormatSweepIssueKind::Indentation
            };
            return (
                kind,
                left_span.start,
                format!(
                    "baseline whitespace {:?} differs from comparison {:?} before token {:?}",
                    left_gap,
                    right_gap,
                    token_label(&left_span.token)
                ),
            );
        }
        let left_token_text = baseline
            .get(left_span.start..left_span.end)
            .unwrap_or_default();
        let right_token_text = comparison
            .get(right_span.start..right_span.end)
            .unwrap_or_default();
        if left_token_text != right_token_text {
            return (
                FormatSweepIssueKind::WhitespaceDependency,
                left_span.start,
                format!(
                    "rendered token text differs: baseline {:?}, comparison {:?}",
                    left_token_text.chars().take(80).collect::<String>(),
                    right_token_text.chars().take(80).collect::<String>()
                ),
            );
        }
        left_cursor = left_span.end;
        right_cursor = right_span.end;
    }
    let left_tail = baseline.get(left_cursor..).unwrap_or_default();
    let right_tail = comparison.get(right_cursor..).unwrap_or_default();
    (
        FormatSweepIssueKind::WhitespaceDependency,
        baseline.len(),
        format!(
            "formatted outputs differ after the final token: baseline trailing {left_tail:?}, comparison trailing {right_tail:?}"
        ),
    )
}

fn format_sweep_run(source: &str, db_type: DatabaseType) -> FormatSweepRun {
    let first = catch_unwind(AssertUnwindSafe(|| {
        SqlEditorWidget::format_for_auto_formatting_with_frame_alignment_audit(
            source,
            Some(db_type),
        )
    }));
    let (formatted, frame_alignment_audit) = match first {
        Ok(result) => result,
        Err(payload) => {
            return FormatSweepRun {
                formatted: source.to_string(),
                checked_lines: 0,
                checked_gaps: 0,
                checked_identifier_case_words: 0,
                checked_frames: 0,
                checked_frame_boundaries: 0,
                checked_frame_depth_symmetries: 0,
                checked_frame_body_items: 0,
                checked_frame_closes: 0,
                managed_frame_kinds: Vec::new(),
                managed_list_owner_kinds: Vec::new(),
                probes: 0,
                issues: vec![FormatSweepIssue::new(
                    FormatSweepIssueKind::FormatPanic,
                    source,
                    0,
                    format!(
                        "first formatting pass panicked: {}",
                        panic_text(payload.as_ref())
                    ),
                )],
            };
        }
    };

    let (checked_lines, checked_gaps, mut issues) =
        format_sweep_audit_first_pass(&formatted, db_type);
    issues.extend(frame_alignment_audit.issues.into_iter().map(|issue| {
        FormatSweepIssue::new(
            FormatSweepIssueKind::FrameAlignment,
            &formatted,
            issue.offset,
            issue.message,
        )
    }));
    if let Some(issue) = format_sweep_audit_token_count(source, &formatted, db_type) {
        issues.push(issue);
    }
    issues.extend(format_sweep_audit_executable_boundaries(
        &formatted, db_type,
    ));
    let (checked_identifier_case_words, identifier_case_issues) =
        format_sweep_audit_identifier_case(source, &formatted, db_type);
    issues.extend(identifier_case_issues);
    let mut probes = 0usize;
    let formatted_document = FormatSweepDocument::new(&formatted, db_type);
    for kind in [
        FormatSweepProbeKind::Reindent,
        FormatSweepProbeKind::CollapseBreaks,
        FormatSweepProbeKind::ExpandInline,
    ] {
        let probe = formatted_document.render_probe(kind);
        if let Some(mut issue) = format_sweep_audit_token_count(&formatted, &probe, db_type) {
            issue.message = format!("{kind:?} whitespace probe: {}", issue.message);
            issues.push(issue);
            continue;
        }
        probes += 1;
        let result = catch_unwind(AssertUnwindSafe(|| {
            SqlEditorWidget::format_for_auto_formatting_with_db_type(&probe, false, Some(db_type))
        }));
        match result {
            Ok(result) if result != formatted => {
                let (layout_kind, offset, message) =
                    whitespace_kind_between_equal_tokens(&formatted, &result, db_type);
                issues.push(FormatSweepIssue::new(
                    FormatSweepIssueKind::WhitespaceDependency,
                    &formatted,
                    offset,
                    format!("{kind:?} whitespace probe: {layout_kind:?}: {message}"),
                ));
            }
            Ok(_) => {}
            Err(payload) => issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::FormatPanic,
                &formatted,
                0,
                format!(
                    "whitespace probe panicked: {}",
                    panic_text(payload.as_ref())
                ),
            )),
        }
    }

    probes += 1;
    match catch_unwind(AssertUnwindSafe(|| {
        SqlEditorWidget::format_for_auto_formatting_with_db_type(&formatted, false, Some(db_type))
    })) {
        Ok(second) if second != formatted => {
            let (layout_kind, offset, message) =
                whitespace_kind_between_equal_tokens(&formatted, &second, db_type);
            issues.push(FormatSweepIssue::new(
                FormatSweepIssueKind::NonIdempotent,
                &formatted,
                offset,
                format!("{layout_kind:?}: {message}"),
            ));
        }
        Ok(_) => {}
        Err(payload) => issues.push(FormatSweepIssue::new(
            FormatSweepIssueKind::FormatPanic,
            &formatted,
            0,
            format!(
                "second formatting pass panicked: {}",
                panic_text(payload.as_ref())
            ),
        )),
    }

    FormatSweepRun {
        formatted,
        checked_lines,
        checked_gaps,
        checked_identifier_case_words,
        checked_frames: frame_alignment_audit.checked_frames,
        checked_frame_boundaries: frame_alignment_audit.checked_frame_boundaries,
        checked_frame_depth_symmetries: frame_alignment_audit.checked_frame_depth_symmetries,
        checked_frame_body_items: frame_alignment_audit.checked_body_items,
        checked_frame_closes: frame_alignment_audit.checked_closes,
        managed_frame_kinds: frame_alignment_audit.managed_frame_kinds,
        managed_list_owner_kinds: frame_alignment_audit.managed_list_owner_kinds,
        probes,
        issues,
    }
}

fn escape_report_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn format_sweep_render_out(
    run: &FormatSweepRun,
    source_label: &str,
    db_type: DatabaseType,
) -> String {
    let mut annotated = run.formatted.clone();
    let mut markers: Vec<(usize, String)> = run
        .issues
        .iter()
        .take(FORMAT_SWEEP_DETAIL_LIMIT)
        .enumerate()
        .map(|(idx, issue)| {
            (
                issue.marker_offset,
                format!("[[FMT:E{:03}:{:?}]]", idx + 1, issue.kind),
            )
        })
        .collect();
    markers.sort_by(|left, right| right.0.cmp(&left.0));
    for (offset, marker) in markers {
        let offset = clamp_char_boundary(&annotated, offset);
        annotated.insert_str(offset, &marker);
    }

    annotated.push_str("\n\n-- =========================================================\n");
    annotated.push_str("-- Auto-format sweep report\n");
    annotated.push_str(&format!("-- source: {source_label}\n"));
    annotated.push_str(&format!("-- db: {db_type:?}\n"));
    annotated.push_str(&format!(
        "-- status: {}\n",
        if run.issues.is_empty() {
            "PASS"
        } else {
            "FAIL"
        }
    ));
    annotated.push_str(&format!(
        "-- checked: lines={} token_gaps={} identifier_case_words={} frames={} frame_boundaries={} frame_depth_symmetries={} frame_body_items={} frame_closes={} probes={}\n",
        run.checked_lines,
        run.checked_gaps,
        run.checked_identifier_case_words,
        run.checked_frames,
        run.checked_frame_boundaries,
        run.checked_frame_depth_symmetries,
        run.checked_frame_body_items,
        run.checked_frame_closes,
        run.probes
    ));
    annotated.push_str(&format!(
        "-- managed_frame_kinds: {:?}\n",
        run.managed_frame_kinds
    ));
    annotated.push_str(&format!(
        "-- managed_list_owner_kinds: {:?}\n",
        run.managed_list_owner_kinds
    ));
    annotated.push_str(&format!("-- issues: total={}\n", run.issues.len()));
    annotated.push_str("-- marker: [[FMT:E...]] marks an invariant error\n");
    for (idx, issue) in run
        .issues
        .iter()
        .take(FORMAT_SWEEP_DETAIL_LIMIT)
        .enumerate()
    {
        annotated.push_str(&format!(
            "--   E{:03} kind={:?} at={}:{} message={}\n",
            idx + 1,
            issue.kind,
            issue.line,
            issue.column,
            escape_report_text(&issue.message)
        ));
    }
    if run.issues.len() > FORMAT_SWEEP_DETAIL_LIMIT {
        annotated.push_str(&format!(
            "--   details truncated: showing {} of {} issues\n",
            FORMAT_SWEEP_DETAIL_LIMIT,
            run.issues.len()
        ));
    }
    annotated.push_str("-- =========================================================\n");
    annotated
}

fn format_sweep_out_path(input_path: &Path) -> PathBuf {
    let name = input_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("format.sql");
    input_path.with_file_name(format!("{name}.format.out"))
}

fn format_sweep_generate_report_for_file(
    input_path: &Path,
    db_type: DatabaseType,
    out_path: &Path,
    fail_on_issue: bool,
) -> FormatSweepRun {
    let source = fs::read_to_string(input_path)
        .unwrap_or_else(|err| panic!("failed to read `{}`: {err}", input_path.display()));
    let run = format_sweep_run(&source, db_type);
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_label = input_path
        .strip_prefix(&manifest)
        .unwrap_or(input_path)
        .display()
        .to_string();
    let report = format_sweep_render_out(&run, &source_label, db_type);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("failed to create `{}`: {err}", parent.display()));
    }
    fs::write(out_path, report)
        .unwrap_or_else(|err| panic!("failed to write `{}`: {err}", out_path.display()));
    if fail_on_issue {
        assert!(
            run.issues.is_empty(),
            "auto-format sweep found {} issues; report written to `{}`",
            run.issues.len(),
            out_path.display()
        );
    }
    run
}

fn format_sweep_db_from_env(value: &str) -> Option<DatabaseType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "oracle" => Some(DatabaseType::Oracle),
        "mysql" => Some(DatabaseType::MySQL),
        "mariadb" | "maria" => Some(DatabaseType::MariaDB),
        _ => None,
    }
}

#[test]
fn formatting_sweep_first_pass_detects_bad_indent_without_second_pass() {
    let source = "SELECT a, b FROM t WHERE a = 1 AND b = 2;";
    let formatted = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        source,
        false,
        Some(DatabaseType::Oracle),
    );
    let bad = formatted.replacen("    AND", "   AND", 1);
    assert_ne!(
        bad, formatted,
        "test input should contain an indented AND line"
    );
    let (_, _, issues) = format_sweep_audit_first_pass(&bad, DatabaseType::Oracle);
    assert!(issues
        .iter()
        .any(|issue| { issue.kind == FormatSweepIssueKind::Indentation }));
}

#[test]
fn formatting_sweep_first_pass_detects_trailing_whitespace() {
    let bad = "SELECT 1 FROM DUAL;  \n";
    let (_, _, issues) = format_sweep_audit_first_pass(bad, DatabaseType::Oracle);
    assert!(issues
        .iter()
        .any(|issue| issue.message == "code line has trailing whitespace"));
}

#[test]
fn formatting_sweep_first_pass_audits_mysql_custom_delimiter_body() {
    let bad = r#"DELIMITER $$
CREATE PROCEDURE p()
BEGIN
  SELECT 1;
END$$
DELIMITER ;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let (_, _, issues) = format_sweep_audit_first_pass(bad, db_type);
        assert!(
            issues.iter().any(|issue| {
                issue.kind == FormatSweepIssueKind::Indentation
                    && issue.message.contains("2 leading spaces")
            }),
            "{db_type:?} custom-delimiter routine body must remain auditable: {issues:#?}"
        );
    }
}

#[test]
fn formatting_sweep_first_pass_protects_multiline_string_in_mysql_routine() {
    let source = r#"DELIMITER $$
CREATE PROCEDURE p()
BEGIN
    SET @message = 'first line
  literal payload';
END$$
DELIMITER ;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let (_, _, issues) = format_sweep_audit_first_pass(source, db_type);
        assert!(
            issues.is_empty(),
            "{db_type:?} multiline string payload must remain protected: {issues:#?}"
        );
    }
}

#[test]
fn formatting_sweep_whitespace_probes_preserve_mysql_custom_delimiters() {
    let source = r#"DELIMITER $$
CREATE PROCEDURE p()
BEGIN
    SELECT 1;
END$$
DELIMITER ;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} custom-delimiter probe issues: {:#?}",
            run.issues
        );
        assert_eq!(
            run.probes, 4,
            "{db_type:?} must run all three whitespace probes plus idempotence"
        );
    }
}

#[test]
fn formatting_sweep_mysql_server_statements_and_commit_modifiers_are_stable() {
    let source = r#"EXECUTE prepared_select USING @prepared_amount, @prepared_category;
START TRANSACTION READ WRITE;
COMMIT AND NO CHAIN;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} server-statement probe issues: {:#?}",
            run.issues
        );
        assert!(
            run.formatted
                .contains("EXECUTE prepared_select USING @prepared_amount, @prepared_category;"),
            "{db_type:?} EXECUTE USING list lost its statement/list ownership:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("START TRANSACTION READ WRITE;"),
            "{db_type:?} START TRANSACTION was split as a client command:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("COMMIT AND NO CHAIN;"),
            "{db_type:?} COMMIT modifier was treated as a boolean condition:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_mysql_xa_and_show_server_statements_are_stable() {
    let source = r#"XA START 'sq-final-xa';
XA END 'sq-final-xa';
XA PREPARE 'sq-final-xa';
XA COMMIT 'sq-final-xa';
SHOW CREATE DATABASE sq_manual_final;
SHOW CREATE TABLE sq_manual_table;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} XA / SHOW server-statement probe issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(run.formatted.contains("XA START 'sq-final-xa';"));
        assert!(run
            .formatted
            .contains("SHOW CREATE DATABASE sq_manual_final;"));
    }
}

#[test]
fn formatting_sweep_oracle_exec_bind_assignment_is_stable() {
    let run = format_sweep_run("EXEC :b_inout := 10", DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "Oracle EXEC bind-assignment probe issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    assert_eq!(run.formatted, "EXEC :b_inout := 10;");
}

#[test]
fn formatting_sweep_oracle_mv_log_and_explain_plan_are_stable() {
    let source = r#"CREATE MATERIALIZED VIEW LOG ON sq_oracle_manual_log
  WITH PRIMARY KEY, ROWID (category, amount)
  INCLUDING NEW VALUES;

EXPLAIN PLAN SET STATEMENT_ID = 'SQ_ORACLE_FINAL' FOR
SELECT category, SUM(amount)
FROM sq_oracle_manual_log
GROUP BY category;"#;

    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "Oracle MV LOG / EXPLAIN PLAN probe issues: {:#?}",
        run.issues
    );
    assert!(
        run.formatted.contains(
            "CREATE MATERIALIZED VIEW LOG ON sq_oracle_manual_log\nWITH PRIMARY KEY,\n    ROWID (category,\n        amount)\nINCLUDING NEW VALUES"
        ),
        "MV LOG clauses lost their statement/list ownership:\n{}",
        run.formatted
    );
    assert!(
        run.formatted.contains("FOR SELECT category,"),
        "EXPLAIN PLAN FOR was treated as a procedural loop:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_oracle_property_graph_tables_keep_structural_ownership() {
    let source = r#"CREATE PROPERTY GRAPH graph_cert
  VERTEX TABLES (
    graph_node KEY (node_id) LABEL node PROPERTIES (node_id, node_name)
  )
  EDGE TABLES (
    graph_edge KEY (edge_id)
      SOURCE KEY (source_node_id) REFERENCES graph_node (node_id)
      DESTINATION KEY (target_node_id) REFERENCES graph_node (node_id)
      LABEL owns PROPERTIES (edge_label, edge_weight)
  );"#;

    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "Oracle PROPERTY GRAPH probe issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(
        run.formatted.contains(
            "CREATE PROPERTY GRAPH graph_cert\n    VERTEX TABLES (graph_node\n            KEY (node_id)\n            LABEL node\n            PROPERTIES (node_id,\n                node_name)\n    )\n    EDGE TABLES (graph_edge\n            KEY (edge_id)\n            SOURCE KEY (source_node_id) REFERENCES graph_node (node_id)\n            DESTINATION KEY (target_node_id) REFERENCES graph_node (node_id)\n            LABEL owns\n            PROPERTIES (edge_label,\n                edge_weight)\n    );"
        ),
        "PROPERTY GRAPH clauses were not rendered as owned structural children:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_mysql_view_algorithm_and_event_body_keep_owners() {
    let source = r#"CREATE OR REPLACE ALGORITHM = MERGE VIEW statement_log_v AS
SELECT id FROM statement_log;

CREATE EVENT syntax_event
  ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 DAY
  ON COMPLETION PRESERVE DISABLE
  COMMENT 'disabled event'
  DO UPDATE statement_log SET amount = amount WHERE id = -1;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} VIEW / EVENT probe issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.formatted
                .contains("CREATE OR REPLACE ALGORITHM = MERGE VIEW statement_log_v AS"),
            "{db_type:?} view algorithm value was treated as MERGE DML:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains(
                "    COMMENT 'disabled event'\n    DO\n        UPDATE statement_log\n        SET amount = amount\n        WHERE id = -1;"
            ),
            "{db_type:?} event DO statement lost its CREATE EVENT body depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_mysql_fixed_phrases_ignore_source_line_breaks() {
    let source = r#"DELIMITER $$
CREATE PROCEDURE p(IN p_condition BOOLEAN, IN p_priority INT)
BEGIN
    IF
        NOT
        p_condition THEN
        SELECT 1;
    END
    IF;
    IF p_priority
        NOT
        BETWEEN 1 AND 9 THEN
        SELECT 2;
    END IF;
END$$
DELIMITER ;"#;

    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} fixed-phrase issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.formatted.contains("IF NOT p_condition THEN")
                && run
                    .formatted
                    .contains("IF p_priority NOT BETWEEN 1 AND 9 THEN")
                && run.formatted.contains("END IF;"),
            "{db_type:?} fixed phrases must be independent of source line breaks:\n{}",
            run.formatted
        );
        assert_eq!(run.probes, 4);
    }
}

#[test]
fn formatting_sweep_oracle_fixed_phrases_ignore_source_line_breaks() {
    let source = r#"BEGIN
    IF
        NOT
        p_condition THEN
        NULL;
    END
    IF;
END;"#;

    let run = format_sweep_run(source, DatabaseType::Oracle);

    assert!(
        run.issues.is_empty(),
        "Oracle fixed-phrase issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(
        run.formatted.contains("IF NOT p_condition THEN") && run.formatted.contains("END IF;"),
        "Oracle fixed phrases must be independent of source line breaks:\n{}",
        run.formatted
    );
    assert_eq!(run.probes, 4);
}

#[test]
fn formatting_sweep_first_pass_detects_token_loss() {
    let source = "SELECT a, b FROM t;";
    let formatted_with_loss = "SELECT a FROM t;";
    let issue = format_sweep_audit_token_count(source, formatted_with_loss, DatabaseType::Oracle)
        .expect("token loss should be reported on the first pass");
    assert_eq!(issue.kind, FormatSweepIssueKind::ItemOrTokenChanged);
}

#[test]
fn formatting_sweep_detects_unterminated_mysql_server_command() {
    let formatted = "DESCRIBE t\n\nSELECT 1;";

    let issues = format_sweep_audit_executable_boundaries(formatted, DatabaseType::MySQL);

    assert_eq!(issues.len(), 1, "unexpected issues: {issues:#?}");
    assert_eq!(issues[0].kind, FormatSweepIssueKind::ExecutableBoundary);
    assert_eq!(issues[0].line, 1);
}

#[test]
fn formatting_sweep_detects_spaced_mysql_account_separator() {
    let formatted = "DROP USER IF EXISTS 'u' @ 'localhost';";

    let issues = format_sweep_audit_executable_boundaries(formatted, DatabaseType::MySQL);

    assert_eq!(issues.len(), 1, "unexpected issues: {issues:#?}");
    assert_eq!(issues[0].kind, FormatSweepIssueKind::ExecutableBoundary);
    assert_eq!(issues[0].column, 25);
}

#[test]
fn formatting_sweep_classifies_layout_divergence_without_keyword_rules() {
    let baseline = "SELECT a\nFROM t;";
    let comparison = "SELECT a FROM t;";
    let (kind, _, _) =
        whitespace_kind_between_equal_tokens(baseline, comparison, DatabaseType::Oracle);
    assert_eq!(kind, FormatSweepIssueKind::LineBreak);
}

#[test]
fn formatting_sweep_simple_sql_converges() {
    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let run = format_sweep_run("SELECT a, b FROM t WHERE a = 1 AND b = 2;", db_type);
        assert!(run.issues.is_empty(), "unexpected issues: {:?}", run.issues);
        assert!(
            run.formatted
                .contains("SELECT a,\n    b\nFROM t\nWHERE a = 1\n    AND b = 2"),
            "SELECT and WHERE should both keep the first child inline and render later siblings at frame depth:\n{}",
            run.formatted
        );
        assert_eq!(run.probes, 4, "three whitespace probes plus idempotence");
    }
}

#[test]
fn formatting_sweep_distinguishes_from_list_children_from_attached_join_frames() {
    let source = "SELECT * FROM first_table a, second_table b JOIN third_table c ON c.id = b.id AND c.active = 1;";

    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} FROM/JOIN frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        let lines: Vec<&str> = run.formatted.lines().collect();
        let from_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("FROM first_table a,"))
            .expect("FROM owner and first child");
        let second_from_child_idx = lines
            .iter()
            .position(|line| line.trim_start() == "second_table b")
            .expect("second FROM-list child");
        let join_idx = lines
            .iter()
            .position(|line| line.trim_start() == "JOIN third_table c")
            .expect("attached JOIN frame");
        let on_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("ON c.id = b.id"))
            .expect("JOIN ON child");
        let and_idx = lines
            .iter()
            .position(|line| line.trim_start() == "AND c.active = 1;")
            .expect("second ON-condition child");

        assert_eq!(
            leading_spaces(lines[second_from_child_idx]),
            leading_spaces(lines[from_idx]).saturating_add(FORMAT_SWEEP_INDENT_WIDTH),
            "the second comma-separated FROM child must use the FROM-list body depth:\n{}",
            run.formatted
        );
        assert_eq!(
            leading_spaces(lines[join_idx]),
            leading_spaces(lines[from_idx]),
            "JOIN must start an attached frame at the FROM clause-boundary depth:\n{}",
            run.formatted
        );
        assert_eq!(
            leading_spaces(lines[on_idx]),
            leading_spaces(lines[join_idx]).saturating_add(FORMAT_SWEEP_INDENT_WIDTH),
            "ON must use the JOIN-frame body depth:\n{}",
            run.formatted
        );
        assert_eq!(
            leading_spaces(lines[and_idx]),
            leading_spaces(lines[on_idx]).saturating_add(FORMAT_SWEEP_INDENT_WIDTH),
            "the second ON-condition child must use the condition-frame body depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_all_child_frames_keep_first_inline_and_break_later_siblings() {
    let source = "WITH first_cte AS (SELECT COALESCE(a, b) AS value FROM t WHERE a = 1 AND b = 2), second_cte AS (SELECT value FROM first_cte) SELECT value, value + 1 FROM second_cte;";

    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );

        assert!(
            run.formatted.starts_with("WITH first_cte AS ("),
            "the first WITH child should remain inline:\n{}",
            run.formatted
        );
        assert!(
            run.formatted
                .lines()
                .any(|line| line.trim_start().starts_with("SELECT COALESCE")),
            "the nested SELECT should keep the query-boundary depth and its first list item inline:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("a,\n")
                && run.formatted.lines().any(|line| {
                    line.trim_start().starts_with('b') && line.trim_end().ends_with(") AS value")
                }),
            "the second function argument should start on its parenthesis-frame depth:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("WHERE a = 1\n")
                && run
                    .formatted
                    .lines()
                    .any(|line| line.trim_start().starts_with("AND b = 2")),
            "the second condition child should start on its condition-frame depth:\n{}",
            run.formatted
        );
        assert!(
            run.formatted
                .lines()
                .any(|line| line.trim_start().starts_with("second_cte AS (")),
            "the second WITH child should start on its WITH-frame depth:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("SELECT value,\n")
                && run
                    .formatted
                    .lines()
                    .any(|line| line.trim_start().starts_with("value + 1")),
            "the second SELECT child should start on its SELECT-frame depth:\n{}",
            run.formatted
        );
    }

    let plsql = format_sweep_run(
        "BEGIN v_total := v_total + (CASE WHEN n = 1 THEN 10 ELSE 0 END); END;",
        DatabaseType::Oracle,
    );
    assert!(
        plsql.issues.is_empty(),
        "PL/SQL parenthesized CASE frame issues: {:#?}\n{}",
        plsql.issues,
        plsql.formatted
    );
    assert!(
        plsql.formatted.contains("+ (CASE\n"),
        "a PL/SQL CASE that is the first parenthesis child must remain inline:\n{}",
        plsql.formatted
    );
}

#[test]
fn formatting_sweep_condition_frame_depth_invariant_covers_db_clause_families() {
    for (db_type, source) in [
        (
            DatabaseType::Oracle,
            "SELECT * FROM emp START WITH manager_id IS NULL AND active = 1 CONNECT BY PRIOR emp_id = manager_id AND active = 1;",
        ),
        (
            DatabaseType::Oracle,
            "SELECT CASE WHEN t.a = 1 AND t.b = 2 THEN 1 ELSE 0 END, COUNT(*) FROM t JOIN u ON u.id = t.id OR u.alt_id = t.id WHERE t.active = 1 AND u.active = 1 GROUP BY t.a, t.b HAVING COUNT(*) > 1 OR SUM(t.a) > 10;",
        ),
        (
            DatabaseType::Oracle,
            "SELECT dept_id, COUNT(*) FROM emp GROUP BY dept_id HAVING COUNT(*) > 1 OR SUM(sal) > 10 QUALIFY ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY emp_id) = 1 AND active = 1;",
        ),
        (
            DatabaseType::Oracle,
            "SELECT * FROM emp MATCH_RECOGNIZE (PARTITION BY dept_id ORDER BY emp_id PATTERN (A B+) DEFINE A AS A.sal > 10 AND A.active = 1 OR A.flag = 'Y');",
        ),
        (
            DatabaseType::Oracle,
            "BEGIN IF a = 1 AND b = 2 THEN NULL; END IF; LOOP EXIT WHEN a = 1 OR b = 2; END LOOP; EXCEPTION WHEN NO_DATA_FOUND OR TOO_MANY_ROWS THEN NULL; END;",
        ),
        (
            DatabaseType::Oracle,
            "CREATE OR REPLACE TRIGGER trg BEFORE INSERT OR UPDATE ON t FOR EACH ROW WHEN (NEW.a = 1 OR NEW.b = 2) BEGIN NULL; END;",
        ),
        (
            DatabaseType::MySQL,
            "SELECT IF(a = 1 AND b = 2 OR c = 3, 'Y', 'N') FROM t;",
        ),
        (
            DatabaseType::MySQL,
            "SELECT CASE WHEN t.a = 1 AND t.b = 2 THEN 1 ELSE 0 END, COUNT(*) FROM t JOIN u ON u.id = t.id OR u.alt_id = t.id WHERE t.active = 1 AND u.active = 1 GROUP BY t.a, t.b HAVING COUNT(*) > 1 OR SUM(t.a) > 10;",
        ),
        (
            DatabaseType::MySQL,
            "CREATE PROCEDURE p() BEGIN IF a = 1 AND b = 2 THEN SET a = 2; END IF; WHILE a = 2 OR b = 3 DO SET a = 3; END WHILE; REPEAT SET a = a + 1; UNTIL a = 4 AND b = 5; END REPEAT; END;",
        ),
        (
            DatabaseType::MariaDB,
            "SELECT IF(a = 1 AND b = 2 OR c = 3, 'Y', 'N') FROM t;",
        ),
        (
            DatabaseType::MariaDB,
            "SELECT CASE WHEN t.a = 1 AND t.b = 2 THEN 1 ELSE 0 END, COUNT(*) FROM t JOIN u ON u.id = t.id OR u.alt_id = t.id WHERE t.active = 1 AND u.active = 1 GROUP BY t.a, t.b HAVING COUNT(*) > 1 OR SUM(t.a) > 10;",
        ),
        (
            DatabaseType::MariaDB,
            "CREATE PROCEDURE p() BEGIN IF a = 1 AND b = 2 THEN SET a = 2; END IF; WHILE a = 2 OR b = 3 DO SET a = 3; END WHILE; REPEAT SET a = a + 1; UNTIL a = 4 AND b = 5; END REPEAT; END;",
        ),
    ] {
        let run = format_sweep_run(source, db_type);
        let invariant_issues: Vec<&FormatSweepIssue> = run
            .issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    FormatSweepIssueKind::FrameAlignment | FormatSweepIssueKind::NonIdempotent
                )
            })
            .collect();
        assert!(
            invariant_issues.is_empty(),
            "{db_type:?} condition-frame issues: {:#?}\n{}",
            invariant_issues,
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_condition_frames_keep_inline_first_child_and_depth_aligned_siblings() {
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());

    for (db_type, source) in FORMAT_SWEEP_FRAME_REGRESSION_CASES {
        let run = format_sweep_run(source, *db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} expanded-frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );

        let lines: Vec<&str> = run.formatted.lines().collect();
        let when_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("WHEN ("))
            .unwrap_or_else(|| panic!("WHEN owner not found:\n{}", run.formatted));
        let nested_select = lines
            .iter()
            .skip(when_idx + 1)
            .find(|line| line.trim_start().starts_with("SELECT COUNT(*)"))
            .copied()
            .unwrap_or_else(|| panic!("nested SELECT not found:\n{}", run.formatted));
        let first_close = lines
            .iter()
            .skip(when_idx + 1)
            .find(|line| line.trim_start().starts_with(") = 6"))
            .copied()
            .unwrap_or_else(|| panic!("first nested close not found:\n{}", run.formatted));
        let and_child = lines
            .iter()
            .skip(when_idx + 1)
            .find(|line| line.trim_start().starts_with("AND ("))
            .copied()
            .unwrap_or_else(|| panic!("AND child not found:\n{}", run.formatted));

        assert_eq!(
            indent(nested_select),
            indent(lines[when_idx]) + 2 * FORMAT_SWEEP_INDENT_WIDTH,
            "the nested query must traverse the condition and parenthesis frame edges:\n{}",
            run.formatted
        );

        assert_eq!(
            indent(first_close),
            indent(lines[when_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "the nested close must return to the inline condition child's stored frame depth:\n{}",
            run.formatted
        );
        assert_eq!(
            indent(and_child),
            indent(lines[when_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "later condition siblings must use the condition frame depth even when the first child is inline:\n{}",
            run.formatted
        );

        let on_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("ON /* join child */"))
            .unwrap_or_else(|| panic!("ON owner not found:\n{}", run.formatted));
        let join_child = lines
            .get(on_idx + 1)
            .copied()
            .unwrap_or_else(|| panic!("ON child not found:\n{}", run.formatted));
        assert_eq!(
            indent(join_child),
            indent(lines[on_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "single line-start condition child must start at owner + 1:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_structural_regressions_remain_frame_managed() {
    let mut managed_frame_kinds = Vec::new();

    for (db_type, source) in FORMAT_SWEEP_STRUCTURAL_REGRESSION_CASES {
        let run = format_sweep_run(source, *db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} structural-frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        for kind in run.managed_frame_kinds {
            if !managed_frame_kinds.contains(&kind) {
                managed_frame_kinds.push(kind);
            }
        }
    }

    for expected in [
        FormatManagedFrameKind::WithCte,
        FormatManagedFrameKind::JoinBody,
        FormatManagedFrameKind::MergeOn,
        FormatManagedFrameKind::AssignmentValue,
        FormatManagedFrameKind::ExecuteImmediate,
    ] {
        assert!(
            managed_frame_kinds.contains(&expected),
            "structural regression cases must exercise {expected:?}: {managed_frame_kinds:?}"
        );
    }
}

#[test]
fn formatting_sweep_inline_query_frame_body_is_one_deeper_than_close() {
    let run = format_sweep_run(
        FORMAT_SWEEP_INLINE_QUERY_FRAME_REGRESSION,
        DatabaseType::Oracle,
    );
    assert!(
        run.issues.is_empty(),
        "inline query-frame issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );

    let lines: Vec<&str> = run.formatted.lines().collect();
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());
    let query_body = lines
        .iter()
        .find(|line| line.trim_start().starts_with("SELECT SUM"))
        .copied()
        .unwrap_or_else(|| panic!("inline query body not found:\n{}", run.formatted));
    let close = lines
        .iter()
        .find(|line| line.trim_start().starts_with(") RETURNING CLOB"))
        .copied()
        .unwrap_or_else(|| panic!("inline query close not found:\n{}", run.formatted));
    assert_eq!(
        indent(query_body),
        indent(close) + FORMAT_SWEEP_INDENT_WIDTH,
        "query-frame body must be one level deeper than its close:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_select_inline_comment_is_not_moved_to_expand_its_frame() {
    let run = format_sweep_run(
        FORMAT_SWEEP_INLINE_SELECT_COMMENT_REGRESSION,
        DatabaseType::Oracle,
    );
    assert!(
        run.issues.is_empty(),
        "inline SELECT-comment issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(
        run.formatted.contains("SELECT /* first child */\n"),
        "SELECT's first-child comment must stay inline:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_comment_forced_first_child_uses_frame_body_depth() {
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());

    for source in [
        "SELECT * FROM sales MODEL MEASURES -- metrics\n(amt) RULES (amt[1] = 1);",
        "INSERT INTO t_log (id, msg) VALUES -- tuple\n(1, 'x');",
    ] {
        let run = format_sweep_run(source, DatabaseType::Oracle);
        assert!(
            run.issues.is_empty(),
            "comment-forced first-child issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        let lines: Vec<&str> = run.formatted.lines().collect();
        let owner_idx = lines
            .iter()
            .position(|line| line.contains("--"))
            .expect("comment-bearing frame owner");
        let child = lines
            .get(owner_idx + 1)
            .expect("first child after the owner comment");
        assert_eq!(
            indent(child),
            indent(lines[owner_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "a comment-forced line-start first child must use frame body depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_inline_child_continuations_use_the_same_frame_depth() {
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());

    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        for source in FORMAT_SWEEP_INLINE_CONTINUATION_REGRESSIONS {
            let run = format_sweep_run(source, db_type);
            assert!(
                run.issues.is_empty(),
                "{db_type:?} continuation-frame issues: {:#?}\n{}",
                run.issues,
                run.formatted
            );
            if source.starts_with("WITH ") {
                assert!(
                    run.managed_list_owner_kinds.contains(&ListOwnerKind::With),
                    "a single inline WITH child must still have a managed frame:\n{}",
                    run.formatted
                );
            }

            let lines: Vec<&str> = run.formatted.lines().collect();
            let owner_idx = lines
                .iter()
                .position(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower.contains("first_value + -- continuation")
                        || lower.contains("first_value = -- continuation")
                })
                .unwrap_or_else(|| panic!("continuation owner not found:\n{}", run.formatted));
            let continuation = lines
                .get(owner_idx.saturating_add(1))
                .copied()
                .unwrap_or_else(|| panic!("continuation line not found:\n{}", run.formatted));
            assert!(
                continuation.trim_start().starts_with("second_value"),
                "unexpected continuation line:\n{}",
                run.formatted
            );
            assert_eq!(
                indent(continuation),
                indent(lines[owner_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
                "an inline child's first rendered continuation must use owner + 1 frame depth:\n{}",
                run.formatted
            );
        }
    }
}

#[test]
fn formatting_sweep_comma_list_frame_depth_invariant_covers_db_clause_families() {
    for (db_type, source, minimum_body_items) in [
        (
            DatabaseType::Oracle,
            r#"SELECT
    CASE WHEN e.active = 1 THEN e.emp_id ELSE 0 END AS active_emp,
    e.dept_id,
    SUM(e.sal) AS total_sal
FROM
    (SELECT emp_id, dept_id, sal, active FROM emp) e,
    dept d
GROUP BY
    e.dept_id,
    d.dept_name
ORDER BY
    SUM(e.sal),
    e.dept_id;"#,
            8,
        ),
        (
            DatabaseType::MySQL,
            r#"UPDATE employee
SET
    salary = salary + 1,
    updated_at = CURRENT_TIMESTAMP,
    active = 1
WHERE employee_id = 1;
INSERT INTO audit_log (employee_id, action_name, created_at)
VALUES
    (1, 'A', CURRENT_TIMESTAMP),
    (2, 'B', CURRENT_TIMESTAMP);"#,
            3,
        ),
        (
            DatabaseType::MariaDB,
            r#"SELECT
    IF(active = 1, employee_id, 0) AS active_emp,
    department_id,
    COUNT(*) AS employee_count
FROM employee
GROUP BY
    department_id,
    active
ORDER BY
    employee_count,
    department_id;"#,
            4,
        ),
    ] {
        let run = format_sweep_run(source, db_type);
        let frame_issues: Vec<&FormatSweepIssue> = run
            .issues
            .iter()
            .filter(|issue| issue.kind == FormatSweepIssueKind::FrameAlignment)
            .collect();
        assert!(
            frame_issues.is_empty(),
            "{db_type:?} comma-list frame issues: {frame_issues:#?}\n{}",
            run.formatted
        );
        assert!(
            run.checked_frame_body_items >= minimum_body_items,
            "{db_type:?} comma-list frames were not fully audited: {run:#?}"
        );
    }
}

#[test]
fn formatting_sweep_syntax_inventory_is_owned_by_typed_frames() {
    let cases: &[(
        &str,
        DatabaseType,
        &str,
        &[FormatManagedFrameKind],
    )] = &[
        (
            "query clauses, comma lists, and conditions",
            DatabaseType::Oracle,
            "SELECT a, b FROM t1, t2 WHERE t1.id = t2.id AND t1.active = 1 ORDER BY a, b;",
            &[
                FormatManagedFrameKind::Query,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "JOIN body and ON-condition children",
            DatabaseType::Oracle,
            "SELECT t1.a, t2.b FROM t1 LEFT JOIN t2 ON t2.id = t1.id AND t2.active = 1;",
            &[
                FormatManagedFrameKind::JoinBody,
                FormatManagedFrameKind::Condition,
                FormatManagedFrameKind::List,
            ],
        ),
        (
            "CTE siblings and set operands",
            DatabaseType::Oracle,
            "WITH a AS (SELECT id, v FROM t1), b AS (SELECT id, v FROM t2) SELECT id, v FROM a UNION ALL SELECT id, v FROM b;",
            &[
                FormatManagedFrameKind::WithCte,
                FormatManagedFrameKind::Parenthesized,
                FormatManagedFrameKind::Query,
                FormatManagedFrameKind::List,
            ],
        ),
        (
            "MERGE branches, conditions, and assignment lists",
            DatabaseType::Oracle,
            "MERGE INTO t USING (SELECT id, a, b FROM s) x ON (t.id = x.id AND t.active = 1) WHEN MATCHED THEN UPDATE SET t.a = x.a, t.b = x.b WHEN NOT MATCHED THEN INSERT (id, a, b) VALUES (x.id, x.a, x.b);",
            &[
                FormatManagedFrameKind::MergeBranch,
                FormatManagedFrameKind::MergeOn,
                FormatManagedFrameKind::Condition,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Parenthesized,
            ],
        ),
        (
            "INSERT ALL branches and row-value lists",
            DatabaseType::Oracle,
            "INSERT ALL WHEN active = 1 AND kind = 'A' THEN INTO ta (id, v) VALUES (id, v) WHEN active = 1 AND kind = 'B' THEN INTO tb (id, v) VALUES (id, v) SELECT id, v, active, kind FROM src;",
            &[
                FormatManagedFrameKind::InsertAll,
                FormatManagedFrameKind::Condition,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Parenthesized,
            ],
        ),
        (
            "MATCH_RECOGNIZE section lists and DEFINE conditions",
            DatabaseType::Oracle,
            "SELECT * FROM sales MATCH_RECOGNIZE (PARTITION BY dept_id, region_id ORDER BY sale_date, sale_id MEASURES FIRST(sale_date) AS first_date, LAST(sale_date) AS last_date PATTERN (A B+) DEFINE A AS A.amount > 10 AND A.active = 1, B AS B.amount > A.amount);",
            &[
                FormatManagedFrameKind::Parenthesized,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "MODEL dimensions, measures, and rules",
            DatabaseType::Oracle,
            "SELECT dept_id, month_id, amount, projected FROM sales MODEL PARTITION BY (dept_id) DIMENSION BY (month_id) MEASURES (amount, projected) RULES (projected[ANY] = amount[CV(month_id)] * 1.1, amount[ANY] = amount[CV(month_id)]);",
            &[
                FormatManagedFrameKind::ModelBody,
                FormatManagedFrameKind::Parenthesized,
                FormatManagedFrameKind::List,
            ],
        ),
        (
            "Oracle conditional-compilation boundaries",
            DatabaseType::Oracle,
            "CREATE OR REPLACE PROCEDURE p IS BEGIN $IF DBMS_DB_VERSION.VERSION >= 19 $THEN NULL; $ELSIF DBMS_DB_VERSION.VERSION >= 12 $THEN NULL; $ELSE NULL; $END NULL; END;",
            &[
                FormatManagedFrameKind::OracleConditionalCompilation,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::PlsqlContext,
            ],
        ),
        (
            "cursor SQL and FORALL body",
            DatabaseType::Oracle,
            "DECLARE CURSOR c IS SELECT id FROM src; BEGIN FORALL i IN 1 .. 3 INSERT INTO dst (id) VALUES (i); FOR r IN (SELECT id FROM src) LOOP BEGIN IF r.id = 1 THEN NULL; ELSIF r.id = 2 THEN NULL; ELSE NULL; END IF; END; END LOOP; END;",
            &[
                FormatManagedFrameKind::CursorSql,
                FormatManagedFrameKind::ForallBody,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "CASE expression branch bodies",
            DatabaseType::Oracle,
            "SELECT CASE WHEN a = 1 AND b = 2 THEN CASE WHEN c = 3 THEN 1 ELSE 2 END ELSE 0 END AS flag_value FROM t;",
            &[
                FormatManagedFrameKind::CaseBranch,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "PL/SQL exception branch bodies",
            DatabaseType::Oracle,
            "BEGIN NULL; EXCEPTION WHEN NO_DATA_FOUND OR TOO_MANY_ROWS THEN NULL; WHEN OTHERS THEN RAISE; END;",
            &[
                FormatManagedFrameKind::ExceptionBranch,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "compound-trigger sections and event alternatives",
            DatabaseType::Oracle,
            "CREATE OR REPLACE TRIGGER trg FOR INSERT OR UPDATE OR DELETE ON t COMPOUND TRIGGER BEFORE STATEMENT IS BEGIN NULL; END BEFORE STATEMENT; AFTER EACH ROW IS BEGIN NULL; END AFTER EACH ROW; END trg;",
            &[
                FormatManagedFrameKind::TriggerHeader,
                FormatManagedFrameKind::CompoundTrigger,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::PlsqlContext,
            ],
        ),
        (
            "dynamic SQL USING and INTO bind lists",
            DatabaseType::Oracle,
            "DECLARE a NUMBER; b NUMBER; c NUMBER; BEGIN EXECUTE IMMEDIATE 'SELECT x, y FROM t WHERE id = :1' INTO a, b USING c; END;",
            &[
                FormatManagedFrameKind::ExecuteImmediate,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::PlsqlContext,
            ],
        ),
        (
            "MySQL handler conditions and compound body",
            DatabaseType::MySQL,
            "CREATE PROCEDURE p() BEGIN DECLARE CONTINUE HANDLER FOR SQLWARNING, SQLEXCEPTION BEGIN SET @a = 1; SET @b = 2; END; SELECT a, b FROM t WHERE a = 1 OR b = 2; END;",
            &[
                FormatManagedFrameKind::MySqlHandlerBody,
                FormatManagedFrameKind::List,
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "MySQL event statement body",
            DatabaseType::MySQL,
            "CREATE EVENT e ON SCHEDULE AT CURRENT_TIMESTAMP DO UPDATE t SET amount = amount WHERE id = 1;",
            &[FormatManagedFrameKind::EventBody],
        ),
        (
            "MariaDB routine control conditions",
            DatabaseType::MariaDB,
            "CREATE PROCEDURE p() BEGIN IF a = 1 AND b = 2 THEN SET a = 2; ELSE SET b = 3; END IF; WHILE a < 10 OR b < 10 DO SET a = a + 1; END WHILE; REPEAT SET b = b + 1; UNTIL b > 10 AND a > 5; END REPEAT; END;",
            &[
                FormatManagedFrameKind::Block,
                FormatManagedFrameKind::Condition,
            ],
        ),
        (
            "DDL structured columns and privilege lists",
            DatabaseType::Oracle,
            "SELECT jt.id, jt.name FROM JSON_TABLE(doc, '$[*]' COLUMNS (id NUMBER PATH '$.id', name VARCHAR2(30) PATH '$.name')) jt; GRANT SELECT, INSERT, UPDATE ON t TO app;",
            &[
                FormatManagedFrameKind::Parenthesized,
                FormatManagedFrameKind::List,
            ],
        ),
    ];

    let mut actual_kinds = Vec::new();
    for (label, db_type, source, expected_kinds) in cases {
        let run = format_sweep_run(source, *db_type);
        let frame_issues: Vec<&FormatSweepIssue> = run
            .issues
            .iter()
            .filter(|issue| issue.kind == FormatSweepIssueKind::FrameAlignment)
            .collect();
        assert!(
            frame_issues.is_empty(),
            "{label} produced frame invariant errors: {frame_issues:#?}\n{}",
            run.formatted
        );
        for expected_kind in *expected_kinds {
            assert!(
                run.managed_frame_kinds.contains(expected_kind),
                "{label} did not create {expected_kind:?}; actual={:?}\n{}",
                run.managed_frame_kinds,
                run.formatted
            );
        }
        for kind in run.managed_frame_kinds {
            if !actual_kinds.contains(&kind) {
                actual_kinds.push(kind);
            }
        }
    }
    actual_kinds.sort_unstable();

    let mut expected_kinds = FormatManagedFrameKind::ALL.to_vec();
    expected_kinds.sort_unstable();
    assert_eq!(
        actual_kinds, expected_kinds,
        "every production frame kind must be exercised by an invariant-checked syntax sample"
    );
}

#[test]
fn formatting_sweep_search_cycle_comma_children_have_dedicated_list_frames() {
    let source = "WITH r (id, parent_id) AS (SELECT 1, 0 FROM DUAL UNION ALL SELECT id + 1, id FROM r WHERE id < 3) SEARCH DEPTH FIRST BY id, parent_id SET traversal_no CYCLE id, parent_id SET cycle_yn TO 'Y' DEFAULT 'N' SELECT id, parent_id FROM r;";
    let run = format_sweep_run(source, DatabaseType::Oracle);

    assert!(
        run.issues.is_empty(),
        "SEARCH/CYCLE frame issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    for kind in [ListOwnerKind::SearchBy, ListOwnerKind::CycleColumns] {
        assert!(
            run.managed_list_owner_kinds.contains(&kind),
            "SEARCH/CYCLE did not create {kind:?}: {:?}\n{}",
            run.managed_list_owner_kinds,
            run.formatted
        );
    }
    assert!(
        run.formatted
            .contains("    SEARCH DEPTH FIRST BY id,\n        parent_id SET traversal_no"),
        "SEARCH siblings should use one list-body depth:\n{}",
        run.formatted
    );
    assert!(
        run.formatted
            .contains("    CYCLE id,\n        parent_id SET cycle_yn"),
        "CYCLE siblings should use one list-body depth:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_with_search_cycle_clauses_continue_the_with_list_child_depth() {
    let source = "WITH a (n) AS (SELECT 1 FROM DUAL UNION ALL SELECT n + 1 FROM a WHERE n < 3) SEARCH DEPTH FIRST BY n SET ord CYCLE n SET cyc TO 'Y' DEFAULT 'N', b AS (SELECT n FROM a) SELECT n FROM b;";
    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "SEARCH/CYCLE continuation issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(
        run.formatted
            .contains("    SEARCH DEPTH FIRST BY n SET ord\n    CYCLE n SET cyc TO 'Y' DEFAULT 'N',\n    b AS ("),
        "SEARCH/CYCLE must continue the recursive CTE at the WITH-list child depth:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_assignment_value_paren_shares_the_owner_edge() {
    let plsql = format_sweep_run(
        "BEGIN v := (SELECT COUNT(*) FROM t WHERE t.id = 1); END;",
        DatabaseType::Oracle,
    );
    assert!(
        plsql.issues.is_empty(),
        "assignment-value paren issues: {:#?}\n{}",
        plsql.issues,
        plsql.formatted
    );
    assert!(
        plsql.formatted.contains(
            "    v := (\n        SELECT COUNT(*)\n        FROM t\n        WHERE t.id = 1\n    );"
        ),
        "a value that is exactly one paren must share the assignment-value depth:\n{}",
        plsql.formatted
    );

    let update = format_sweep_run(
        "UPDATE t SET a = (SELECT MAX(x) FROM s WHERE s.id = t.id), b = 2 WHERE t.id = 1;",
        DatabaseType::Oracle,
    );
    assert!(
        update.issues.is_empty(),
        "SET assignment-value paren issues: {:#?}\n{}",
        update.issues,
        update.formatted
    );
    assert!(
        update
            .formatted
            .contains("SET a = (\n        SELECT MAX (x)\n        FROM s\n        WHERE s.id = t.id\n    ),\n    b = 2"),
        "a SET value that is exactly one paren must share the assignment-value depth:\n{}",
        update.formatted
    );

    let operand = format_sweep_run(
        "CREATE PROCEDURE p() BEGIN SET @v = ((@a + @b) MOD 3) + 1; END;",
        DatabaseType::MariaDB,
    );
    assert!(
        operand.issues.is_empty(),
        "operand paren issues: {:#?}\n{}",
        operand.issues,
        operand.formatted
    );
    assert!(
        operand
            .formatted
            .contains("SET @v = ((@a + @b) MOD 3) + 1;"),
        "a paren that is only the first operand keeps its own edge:\n{}",
        operand.formatted
    );
}

#[test]
fn formatting_sweep_mysql_routine_parameter_type_arguments_close_inline() {
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(
            "CREATE FUNCTION f(a DECIMAL(12, 2), b DECIMAL(14, 2)) RETURNS DECIMAL(18, 2) DETERMINISTIC BEGIN RETURN a + b; END;",
            db_type,
        );
        assert!(
            run.issues.is_empty(),
            "{db_type:?} routine parameter issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.formatted.contains("a DECIMAL(12, 2),"),
            "{db_type:?}: a fixed type modifier is not a comma-list and must stay inline:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("b DECIMAL(14, 2)\n)")
                && run.formatted.contains("RETURNS DECIMAL(18, 2)"),
            "{db_type:?}: parameter and return type modifiers must remain compact:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_fixed_type_modifiers_are_not_comma_lists() {
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let run = format_sweep_run(
            "CREATE PROCEDURE p() BEGIN DECLARE v DECIMAL(18, 2); SELECT CAST(v AS NUMERIC(14, 4)) INTO v; END;",
            db_type,
        );
        assert!(
            run.issues.is_empty(),
            "{db_type:?} fixed type-modifier issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.formatted.contains("DECLARE v DECIMAL(18, 2);"),
            "{db_type:?}: declaration precision/scale must stay inline:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("CAST(v AS NUMERIC(14, 4))"),
            "{db_type:?}: CAST precision/scale must stay inline:\n{}",
            run.formatted
        );
    }

    let oracle = format_sweep_run(
        "DECLARE v NUMBER(18, 2); BEGIN SELECT CAST(1 AS NUMBER(14, 4)) INTO v FROM dual; END;",
        DatabaseType::Oracle,
    );
    assert!(
        oracle.issues.is_empty(),
        "Oracle fixed type-modifier issues: {:#?}\n{}",
        oracle.issues,
        oracle.formatted
    );
    assert!(
        oracle.formatted.contains("v NUMBER (18, 2);"),
        "Oracle declaration precision/scale must stay inline:\n{}",
        oracle.formatted
    );
    assert!(
        oracle.formatted.contains("CAST (1 AS NUMBER (14, 4))"),
        "Oracle CAST precision/scale must stay inline:\n{}",
        oracle.formatted
    );
}

#[test]
fn formatting_sweep_assignment_keeps_case_first_child_inline() {
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let update = format_sweep_run(
            "UPDATE jobs j SET j.job_status = CASE WHEN j.score >= 90 THEN 'READY' ELSE 'HOLD' END, j.priority = CASE WHEN j.score >= 90 THEN 1 ELSE 2 END WHERE j.id = 1;",
            db_type,
        );
        assert!(
            update.issues.is_empty(),
            "{db_type:?} assignment CASE issues: {:#?}\n{}",
            update.issues,
            update.formatted
        );
        assert!(
            update.formatted.contains("j.job_status = CASE\n"),
            "{db_type:?}: the first assignment child CASE must stay on its owner line:\n{}",
            update.formatted
        );
        assert!(
            update.formatted.contains("j.priority = CASE\n"),
            "{db_type:?}: later assignment siblings use the same first-child rule:\n{}",
            update.formatted
        );
    }

    let oracle = format_sweep_run(
        "BEGIN v_result := CASE WHEN v_flag = 1 THEN 10 ELSE 20 END; END;",
        DatabaseType::Oracle,
    );
    assert!(
        oracle.issues.is_empty(),
        "Oracle assignment CASE issues: {:#?}\n{}",
        oracle.issues,
        oracle.formatted
    );
    assert!(
        oracle.formatted.contains("v_result := CASE\n"),
        "Oracle := assignment must keep its first CASE child inline:\n{}",
        oracle.formatted
    );
}

#[test]
fn formatting_sweep_mysql_non_parenthesized_lists_have_dedicated_frames() {
    let source = "CREATE PROCEDURE p() BEGIN DECLARE v_state CHAR(5); DECLARE v_errno INT; DECLARE v_message TEXT; GET STACKED DIAGNOSTICS CONDITION 1 v_state = RETURNED_SQLSTATE, v_errno = MYSQL_ERRNO, v_message = MESSAGE_TEXT; END; DELETE d, r FROM document d JOIN reading r ON r.id = d.id;";
    let run = format_sweep_run(source, DatabaseType::MySQL);

    assert!(
        run.issues.is_empty(),
        "MySQL non-parenthesized list issues: {:#?}\n{}",
        run.issues,
        run.formatted
    );
    for kind in [
        ListOwnerKind::DiagnosticsItems,
        ListOwnerKind::DeleteTargets,
    ] {
        assert!(
            run.managed_list_owner_kinds.contains(&kind),
            "MySQL syntax did not create {kind:?}: {:?}\n{}",
            run.managed_list_owner_kinds,
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_additional_non_parenthesized_child_lists_have_typed_frames() {
    let cases: &[(DatabaseType, &str, &[ListOwnerKind])] = &[
        (
            DatabaseType::Oracle,
            "SELECT * FROM sales MATCH_RECOGNIZE (ORDER BY sale_id SUBSET ab = (A, B), cd = (C, D) PATTERN (A B C D) DEFINE A AS amount > 0); SELECT a, b FROM t FOR UPDATE OF a, b; CREATE OR REPLACE TRIGGER trg BEFORE UPDATE OF a, b ON t FOLLOWS trg_a, trg_b BEGIN NULL; END; GRANT SELECT, UPDATE ON t TO app_a, app_b; ALTER TABLE t ADD c NUMBER, ADD d NUMBER; LOCK TABLE t_a, t_b IN EXCLUSIVE MODE; FLASHBACK TABLE t_a, t_b TO SCN 1;",
            &[
                ListOwnerKind::Subset,
                ListOwnerKind::ForUpdateColumns,
                ListOwnerKind::TriggerUpdateColumns,
                ListOwnerKind::TriggerOrdering,
                ListOwnerKind::GrantPrivileges,
                ListOwnerKind::GrantGrantees,
                ListOwnerKind::AlterActions,
                ListOwnerKind::LockTables,
                ListOwnerKind::FlashbackTargets,
            ],
        ),
        (
            DatabaseType::MySQL,
            "UPDATE t1, t2 SET t1.v = 1, t2.v = 2; ALTER TABLE t1 ADD a INT, ADD b INT; DROP TABLE IF EXISTS old_a, old_b; RENAME TABLE old_c TO new_c, old_d TO new_d; LOCK TABLES new_c READ, new_d WRITE; ANALYZE TABLE new_c, new_d; CREATE USER user_a IDENTIFIED BY 'x', user_b IDENTIFIED BY 'y'; CREATE PROCEDURE p() BEGIN DECLARE v_a, v_b INT; DO v_a, v_b; END;",
            &[
                ListOwnerKind::UpdateTargets,
                ListOwnerKind::AlterActions,
                ListOwnerKind::DropTargets,
                ListOwnerKind::RenamePairs,
                ListOwnerKind::LockTables,
                ListOwnerKind::MaintenanceTables,
                ListOwnerKind::AccountTargets,
                ListOwnerKind::DeclarationNames,
                ListOwnerKind::DoExpressions,
            ],
        ),
        (
            DatabaseType::MariaDB,
            "UPDATE t1, t2 SET t1.v = 1, t2.v = 2; ALTER TABLE t1 ADD a INT, ADD b INT; CHECK TABLE t1, t2; REPAIR TABLE t1, t2; GRANT role_a, role_b TO user_a, user_b; CREATE PROCEDURE p() BEGIN DECLARE v_a, v_b INT; DO v_a, v_b; END;",
            &[
                ListOwnerKind::UpdateTargets,
                ListOwnerKind::AlterActions,
                ListOwnerKind::MaintenanceTables,
                ListOwnerKind::GrantPrivileges,
                ListOwnerKind::GrantGrantees,
                ListOwnerKind::DeclarationNames,
                ListOwnerKind::DoExpressions,
            ],
        ),
    ];

    for (db_type, source, expected_kinds) in cases {
        let run = format_sweep_run(source, *db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} additional list-frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        for expected_kind in *expected_kinds {
            assert!(
                run.managed_list_owner_kinds.contains(expected_kind),
                "{db_type:?} syntax did not create {expected_kind:?}: {:?}\n{}",
                run.managed_list_owner_kinds,
                run.formatted
            );
        }
    }
}

#[test]
fn formatting_sweep_parenthesized_semantic_child_lists_have_typed_frames() {
    let cases: &[(DatabaseType, &str, &[ListOwnerKind])] = &[
        (
            DatabaseType::Oracle,
            "SELECT jt.id, jt.name FROM JSON_TABLE(doc, '$[*]' COLUMNS (id NUMBER PATH '$.id', name VARCHAR2(30) PATH '$.name')) jt; SELECT LISTAGG(name, ',') WITHIN GROUP (ORDER BY name, id) FROM t; SELECT MAX(name) KEEP (DENSE_RANK FIRST ORDER BY score, id) FROM t; SELECT * FROM sales PIVOT (SUM(amount) AS total, COUNT(*) AS row_count FOR quarter IN (1 AS q1, 2 AS q2)); SELECT dept_id, month_id, amount, projected FROM sales MODEL PARTITION BY (dept_id, region_id) DIMENSION BY (month_id, channel_id) MEASURES (amount, projected) RULES (projected[ANY] = amount[CV(month_id)] * 1.1, amount[ANY] = amount[CV(month_id)]);",
            &[
                ListOwnerKind::StructuredTableArguments,
                ListOwnerKind::StructuredColumns,
                ListOwnerKind::Order,
                ListOwnerKind::PivotAggregates,
                ListOwnerKind::Partition,
                ListOwnerKind::Dimension,
                ListOwnerKind::Measures,
                ListOwnerKind::ModelRules,
            ],
        ),
        (
            DatabaseType::MySQL,
            "SELECT jt.id, jt.name FROM JSON_TABLE(doc, '$[*]' COLUMNS (id INT PATH '$.id', name VARCHAR(30) PATH '$.name')) AS jt;",
            &[
                ListOwnerKind::StructuredTableArguments,
                ListOwnerKind::StructuredColumns,
            ],
        ),
        (
            DatabaseType::MariaDB,
            "SELECT jt.id, jt.name FROM JSON_TABLE(doc, '$[*]' COLUMNS (id INT PATH '$.id', name VARCHAR(30) PATH '$.name')) AS jt;",
            &[
                ListOwnerKind::StructuredTableArguments,
                ListOwnerKind::StructuredColumns,
            ],
        ),
    ];

    for (db_type, source, expected_kinds) in cases {
        let run = format_sweep_run(source, *db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} parenthesized semantic list-frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        for expected_kind in *expected_kinds {
            assert!(
                run.managed_list_owner_kinds.contains(expected_kind),
                "{db_type:?} syntax did not create {expected_kind:?}: {:?}\n{}",
                run.managed_list_owner_kinds,
                run.formatted
            );
        }
    }
}

#[test]
fn formatting_sweep_list_owner_inventory_covers_every_typed_variant() {
    let cases = [
        (
            DatabaseType::Oracle,
            "WITH a AS (SELECT id, dept_id FROM t), b AS (SELECT id, dept_id FROM a) SELECT a.id, b.id INTO v_a, v_b FROM a, b GROUP BY a.id, b.id ORDER BY a.id, b.id FOR UPDATE OF a.id, b.id;",
        ),
        (
            DatabaseType::Oracle,
            "SELECT id, SUM(amount) OVER w FROM sales WINDOW w AS (PARTITION BY dept_id, region_id ORDER BY sale_day, id); UPDATE t SET a = 1, b = 2 RETURNING a, b INTO v_a, v_b; INSERT INTO t (a, b) VALUES (1, 2), (3, 4);",
        ),
        (
            DatabaseType::Oracle,
            "MERGE INTO t USING (SELECT id, a, b FROM s) x ON (t.id = x.id) WHEN MATCHED THEN UPDATE SET t.a = x.a, t.b = x.b;",
        ),
        (
            DatabaseType::Oracle,
            "SELECT * FROM sales MATCH_RECOGNIZE (PARTITION BY dept_id, region_id ORDER BY sale_id MEASURES FIRST(amount) AS first_amount, LAST(amount) AS last_amount SUBSET ab = (A, B), cd = (C, D) PATTERN (A B C D) DEFINE A AS A.amount > 0, B AS B.amount > A.amount);",
        ),
        (
            DatabaseType::Oracle,
            "SELECT dept_id, month_id, amount, projected FROM sales MODEL PARTITION BY (dept_id, region_id) DIMENSION BY (month_id, channel_id) MEASURES (amount, projected) RULES (projected[ANY] = amount[CV(month_id)], amount[ANY] = projected[CV(month_id)]); SELECT * FROM sales PIVOT (SUM(amount) AS total, COUNT(*) AS row_count FOR quarter IN (1 AS q1, 2 AS q2));",
        ),
        (
            DatabaseType::Oracle,
            "SELECT jt.id, jt.name FROM JSON_TABLE(doc, '$[*]' COLUMNS (id NUMBER PATH '$.id', name VARCHAR2(30) PATH '$.name')) jt; GRANT SELECT, UPDATE ON t TO app_a, app_b; CREATE OR REPLACE TRIGGER trg BEFORE UPDATE OF a, b ON t BEGIN NULL; END;",
        ),
        (
            DatabaseType::Oracle,
            "ALTER TABLE t ADD a NUMBER, ADD b NUMBER; LOCK TABLE t_a, t_b IN EXCLUSIVE MODE; FLASHBACK TABLE t_a, t_b TO SCN 1; CREATE OR REPLACE TRIGGER trg_order BEFORE INSERT ON t FOLLOWS trg_a, trg_b BEGIN NULL; END; CREATE MATERIALIZED VIEW LOG ON t WITH PRIMARY KEY, ROWID INCLUDING NEW VALUES;",
        ),
        (
            DatabaseType::Oracle,
            "WITH r (id, parent_id) AS (SELECT 1, 0 FROM DUAL UNION ALL SELECT id + 1, id FROM r WHERE id < 3) SEARCH DEPTH FIRST BY id, parent_id SET traversal_no CYCLE id, parent_id SET cycle_yn TO 'Y' DEFAULT 'N' SELECT id, parent_id FROM r;",
        ),
        (
            DatabaseType::Oracle,
            "CREATE PROPERTY GRAPH graph_cert VERTEX TABLES (graph_node KEY (node_id) PROPERTIES (node_id, node_name));",
        ),
        (
            DatabaseType::MySQL,
            "CREATE PROCEDURE p() BEGIN DECLARE v_state CHAR(5); DECLARE v_errno INT; DECLARE v_a, v_b INT; DECLARE CONTINUE HANDLER FOR SQLWARNING, SQLEXCEPTION SET @handled = 1; GET STACKED DIAGNOSTICS CONDITION 1 v_state = RETURNED_SQLSTATE, v_errno = MYSQL_ERRNO; DO v_a, v_b; END; DELETE d, r FROM document d JOIN reading r ON r.id = d.id;",
        ),
        (
            DatabaseType::MySQL,
            "UPDATE t1, t2 SET t1.v = 1, t2.v = 2; ALTER TABLE t1 ADD a INT, ADD b INT; DROP TABLE IF EXISTS old_a, old_b; RENAME TABLE old_c TO new_c, old_d TO new_d; LOCK TABLES new_c READ, new_d WRITE; ANALYZE TABLE new_c, new_d; CREATE USER user_a IDENTIFIED BY 'x', user_b IDENTIFIED BY 'y';",
        ),
    ];

    let mut actual_kinds = Vec::new();
    for (db_type, source) in cases {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} complete list-owner inventory issues: {:#?}\nmanaged={:?}\n{}",
            run.issues,
            run.managed_list_owner_kinds,
            run.formatted
        );
        for kind in run.managed_list_owner_kinds {
            if !actual_kinds.contains(&kind) {
                actual_kinds.push(kind);
            }
        }
    }
    actual_kinds.sort_unstable();

    let mut expected_kinds = ListOwnerKind::ALL.to_vec();
    expected_kinds.sort_unstable();
    assert_eq!(
        actual_kinds, expected_kinds,
        "every production list-owner variant must be exercised by an invariant-checked syntax sample"
    );
}

#[test]
fn formatting_sweep_with_line_start_children_use_owner_plus_one_depth() {
    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let source = r#"WITH first_cte AS (SELECT 1 AS id),
    /* sibling */
    second_cte AS (SELECT id FROM first_cte),
    third_cte AS (SELECT id FROM second_cte)
SELECT id FROM third_cte;"#;
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} WITH child-depth issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.formatted.starts_with("WITH first_cte AS ("),
            "{db_type:?} first CTE should remain inline with WITH:\n{}",
            run.formatted
        );
        assert!(
            run.formatted
                .contains("),\n    /* sibling */\n    second_cte AS ("),
            "{db_type:?} comment and second CTE should share WITH owner+1 depth:\n{}",
            run.formatted
        );
        assert!(
            run.formatted.contains("),\n    third_cte AS ("),
            "{db_type:?} later CTE siblings should remain on WITH owner+1 depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_single_inline_with_child_keeps_both_frame_edges() {
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());

    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let run = format_sweep_run("WITH a AS (SELECT 1 FROM dual) SELECT * FROM a;", db_type);
        assert!(
            run.issues.is_empty(),
            "{db_type:?} single-CTE frame issues: {:#?}\n{}",
            run.issues,
            run.formatted
        );
        assert!(
            run.managed_list_owner_kinds.contains(&ListOwnerKind::With),
            "{db_type:?} single inline CTE must still create a WITH list frame:\n{}",
            run.formatted
        );

        let lines: Vec<&str> = run.formatted.lines().collect();
        let with_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("WITH a AS ("))
            .unwrap_or_else(|| panic!("WITH owner not found:\n{}", run.formatted));
        let cte_select_idx = lines
            .iter()
            .enumerate()
            .skip(with_idx.saturating_add(1))
            .find(|(_, line)| line.trim_start().starts_with("SELECT 1"))
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| panic!("CTE SELECT not found:\n{}", run.formatted));
        let close_idx = lines
            .iter()
            .enumerate()
            .skip(cte_select_idx.saturating_add(1))
            .find(|(_, line)| line.trim_start() == ")")
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| panic!("CTE close not found:\n{}", run.formatted));
        let outer_select_idx = lines
            .iter()
            .enumerate()
            .skip(close_idx.saturating_add(1))
            .find(|(_, line)| line.trim_start().starts_with("SELECT *"))
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| panic!("outer SELECT not found:\n{}", run.formatted));

        assert_eq!(
            indent(lines[cte_select_idx]),
            indent(lines[with_idx]) + 2 * FORMAT_SWEEP_INDENT_WIDTH,
            "{db_type:?} CTE query must traverse WITH and paren frame edges:\n{}",
            run.formatted
        );

        assert_eq!(
            indent(lines[close_idx]),
            indent(lines[with_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "{db_type:?} CTE close must align with the CTE child owner depth:\n{}",
            run.formatted
        );
        assert_eq!(
            indent(lines[outer_select_idx]),
            indent(lines[with_idx]),
            "{db_type:?} main query must return to the WITH owner depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_audits_nested_parenthesized_list_alignment() {
    let source = r#"CALL sp_assert(
    (
        SELECT COUNT(*)
        FROM task_log
    ),
    expected_count,
    'count mismatch'
);"#;
    let run = format_sweep_run(source, DatabaseType::MariaDB);

    assert!(
        run.issues.is_empty(),
        "unexpected issues: {:?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(run.checked_frames >= 2, "{run:#?}");
    assert!(run.checked_frame_body_items >= 3, "{run:#?}");
    assert!(run.checked_frame_closes >= 1, "{run:#?}");
}

#[test]
fn formatting_sweep_keeps_query_order_keys_out_of_direct_paren_body_audit() {
    let source = r#"CALL mb_assert_2(
    (
        SELECT asset_id
        FROM mb_asset
        ORDER BY VEC_DISTANCE_COSINE(embedding, VEC_FromText('[1,1,1]')),
            asset_id
        LIMIT 1
    ) = 2,
    'nearest vector asset'
);"#;
    let run = format_sweep_run(source, DatabaseType::MariaDB);

    assert!(
        run.issues.is_empty(),
        "query clauses inside a parenthesized CALL argument are not direct paren siblings: {:?}\n{}",
        run.issues,
        run.formatted
    );
    assert!(run.checked_frames >= 4, "{run:#?}");
}

#[test]
fn formatting_sweep_preserves_conditional_compilation_trailing_comments() {
    let source = r#"CREATE OR REPLACE PROCEDURE cc_comment IS
BEGIN
    $IF DBMS_DB_VERSION.VERSION >= 12 $THEN -- then comment
        NULL;
    $ELSE -- else comment
        NULL;
    $END -- end comment
    NULL;
END cc_comment;"#;
    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "unexpected issues: {:?}\n{}",
        run.issues,
        run.formatted
    );
}

#[test]
fn formatting_sweep_audits_function_local_returning_option_depth() {
    let cases = [
        (
            DatabaseType::Oracle,
            "WITH base_emp AS (SELECT e.emp_id, JSON_VALUE(e.json_profile, '$.level' RETURNING VARCHAR2(30)) AS profile_level FROM qt_fmt_emp e) SELECT * FROM base_emp;",
        ),
        (
            DatabaseType::MySQL,
            "WITH settings AS (SELECT JSON_OBJECT('start', '2026-07-01') AS config), params AS (SELECT CAST(JSON_VALUE(config, '$.start' RETURNING CHAR(10) DEFAULT '2026-01-01' ON EMPTY) AS DATE) AS start_day FROM settings) SELECT * FROM params;",
        ),
    ];

    for (db_type, source) in cases {
        let run = format_sweep_run(source, db_type);
        assert!(
            run.issues.is_empty(),
            "unexpected {db_type:?} issues: {:?}\n{}",
            run.issues,
            run.formatted
        );
        let lines: Vec<&str> = run.formatted.lines().collect();
        let path_idx = lines
            .iter()
            .position(|line| line.contains("'$.") && !line.contains("RETURNING"))
            .unwrap_or_else(|| panic!("split JSON path line missing:\n{}", run.formatted));
        let returning_idx = lines
            .iter()
            .position(|line| line.trim_start().starts_with("RETURNING"))
            .unwrap_or_else(|| panic!("RETURNING option line missing:\n{}", run.formatted));
        assert_eq!(
            leading_spaces(lines[path_idx]),
            leading_spaces(lines[returning_idx]),
            "function-local RETURNING must stay at its paren sibling depth:\n{}",
            run.formatted
        );
    }
}

#[test]
fn formatting_sweep_audits_conditional_branch_call_argument_depth() {
    let source = "CREATE OR REPLACE PROCEDURE p IS BEGIN $IF DBMS_DB_VERSION.VERSION >= 12 $THEN AUDIT('m', 'then', 'yes'); $ELSE AUDIT('m', 'else', 'no'); $END AUDIT('m', 'done', 'done'); END;";
    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "unexpected issues: {:?}\n{}",
        run.issues,
        run.formatted
    );
    let lines: Vec<&str> = run.formatted.lines().collect();
    let else_idx = lines
        .iter()
        .position(|line| line.trim_start() == "$ELSE")
        .expect("$ELSE line");
    let else_call_idx = lines
        .iter()
        .enumerate()
        .skip(else_idx + 1)
        .find_map(|(idx, line)| line.trim_start().starts_with("AUDIT ('m',").then_some(idx))
        .unwrap_or_else(|| panic!("$ELSE call missing:\n{}", run.formatted));
    let else_arg_idx = lines
        .iter()
        .enumerate()
        .skip(else_call_idx + 1)
        .find_map(|(idx, line)| line.trim_start().starts_with("'else',").then_some(idx))
        .unwrap_or_else(|| panic!("$ELSE call argument missing:\n{}", run.formatted));
    assert_eq!(
        leading_spaces(lines[else_arg_idx]),
        leading_spaces(lines[else_call_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
        "$ELSE call arguments must remain inside the call paren frame:\n{}",
        run.formatted
    );
}

#[test]
fn formatting_sweep_audits_statement_sibling_after_trailing_comment() {
    let source = "BEGIN\n  p(7, p_out => v_out, p_n => v_n); -- omitted\n  DBMS_OUTPUT.PUT_LINE(v_out);\nEND;\n/";
    let (formatted, audit) = SqlEditorWidget::format_for_auto_formatting_with_frame_alignment_audit(
        source,
        Some(DatabaseType::Oracle),
    );
    assert!(
        audit.issues.is_empty(),
        "unexpected issues: {:?}\n{}",
        audit.issues,
        formatted
    );
    let lines: Vec<&str> = formatted.lines().collect();
    let call_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("p (7,"))
        .expect("first call line");
    let sibling_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("DBMS_OUTPUT.PUT_LINE"))
        .expect("statement sibling line");
    assert_eq!(
        leading_spaces(lines[sibling_idx]),
        leading_spaces(lines[call_idx]),
        "a trailing comment must not carry the closed call frame into the next statement:\n{}",
        formatted
    );
}

#[test]
fn formatting_sweep_audits_multiline_standalone_routine_header_close() {
    let source = "CREATE OR REPLACE PROCEDURE p(p_a IN NUMBER, p_b IN VARCHAR2, p_c IN CLOB) IS PRAGMA AUTONOMOUS_TRANSACTION; BEGIN NULL; END;";
    let run = format_sweep_run(source, DatabaseType::Oracle);
    assert!(
        run.issues.is_empty(),
        "unexpected issues: {:?}\n{}",
        run.issues,
        run.formatted
    );
    let lines: Vec<&str> = run.formatted.lines().collect();
    let header_idx = lines
        .iter()
        .position(|line| line.starts_with("CREATE OR REPLACE PROCEDURE"))
        .expect("routine header");
    let pragma_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PRAGMA"))
        .expect("routine declaration");
    let begin_idx = lines
        .iter()
        .position(|line| line.trim_start() == "BEGIN")
        .expect("routine BEGIN");
    let end_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END;")
        .expect("routine END");
    assert_eq!(
        leading_spaces(lines[pragma_idx]),
        leading_spaces(lines[header_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
        "routine declarations must be one frame below the header:\n{}",
        run.formatted
    );
    assert_eq!(
        leading_spaces(lines[begin_idx]),
        leading_spaces(lines[header_idx])
    );
    assert_eq!(
        leading_spaces(lines[end_idx]),
        leading_spaces(lines[header_idx])
    );
}

#[test]
fn formatting_sweep_reports_unclosed_conditional_compilation_frame() {
    let source = r#"CREATE OR REPLACE PROCEDURE cc_unclosed IS
BEGIN
    $IF DBMS_DB_VERSION.VERSION >= 19 $THEN
        NULL;
END cc_unclosed;"#;
    let run = format_sweep_run(source, DatabaseType::Oracle);

    assert!(
        run.issues.iter().any(|issue| {
            issue.kind == FormatSweepIssueKind::FrameAlignment
                && issue.message.contains("without a matching close boundary")
        }),
        "missing conditional-compilation close should be reported: {run:#?}"
    );
}

#[test]
fn formatting_sweep_report_uses_distinct_format_out_suffix() {
    let input = PathBuf::from("test/test1.txt");
    assert_eq!(
        format_sweep_out_path(&input),
        PathBuf::from("test/test1.txt.format.out")
    );
}

#[test]
fn formatting_sweep_report_marks_first_pass_issue() {
    let run = FormatSweepRun {
        formatted: "SELECT 1 FROM DUAL;".to_string(),
        checked_lines: 1,
        checked_gaps: 1,
        checked_identifier_case_words: 0,
        checked_frames: 0,
        checked_frame_boundaries: 0,
        checked_frame_depth_symmetries: 0,
        checked_frame_body_items: 0,
        checked_frame_closes: 0,
        managed_frame_kinds: Vec::new(),
        managed_list_owner_kinds: Vec::new(),
        probes: 0,
        issues: vec![FormatSweepIssue::new(
            FormatSweepIssueKind::LineBreak,
            "SELECT 1 FROM DUAL;",
            9,
            "missing line break".to_string(),
        )],
    };
    let report = format_sweep_render_out(&run, "test.sql", DatabaseType::Oracle);
    assert!(report.contains("[[FMT:E001:LineBreak]]"));
    assert!(report.contains("-- status: FAIL"));
}

#[test]
fn formatter_structural_depth_has_no_parallel_named_state() {
    let formatter_source = include_str!("formatter.rs");
    let forbidden_names = [
        ["indent", "level"].join("_"),
        ["active", "frame", "depth"].join("_"),
        ["previous", "frame", "depth"].join("_"),
    ];

    for forbidden_name in forbidden_names {
        assert!(
            !formatter_source.contains(&forbidden_name),
            "formatter structural depth must come from live frames, not parallel state `{forbidden_name}`"
        );
    }
}

#[test]
#[ignore = "generates an auto-format sweep report; run explicitly"]
fn formatting_sweep_generate_out_report_from_env() {
    let Some(input) = std::env::var_os("SPACE_QUERY_FORMAT_SWEEP_FILE") else {
        return;
    };
    let input_path = PathBuf::from(input);
    let db_type = std::env::var("SPACE_QUERY_FORMAT_SWEEP_DB")
        .ok()
        .and_then(|value| format_sweep_db_from_env(&value))
        .unwrap_or_else(|| {
            let path = input_path.to_string_lossy().to_ascii_lowercase();
            if path.contains("test_mariadb") {
                DatabaseType::MariaDB
            } else if path.contains("test_mysql") {
                DatabaseType::MySQL
            } else {
                DatabaseType::Oracle
            }
        });
    let out_path = std::env::var_os("SPACE_QUERY_FORMAT_SWEEP_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| format_sweep_out_path(&input_path));
    format_sweep_generate_report_for_file(&input_path, db_type, &out_path, true);
}

#[test]
#[ignore = "audits every SQL fixture and writes reports under target/format-sweep"]
fn formatting_sweep_all_files_generate_out_report() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_root = manifest.join("target/format-sweep");
    let mut failures = Vec::new();
    let mut checked_files = 0usize;
    let mut checked_regressions = 0usize;
    let mut checked_identifier_case_words = 0usize;
    let mut checked_frames = 0usize;
    let mut checked_frame_boundaries = 0usize;
    let mut checked_frame_depth_symmetries = 0usize;
    let mut checked_frame_body_items = 0usize;
    let mut checked_frame_closes = 0usize;
    let mut managed_frame_kinds = Vec::new();
    let mut managed_list_owner_kinds = Vec::new();

    let built_in_regressions = FORMAT_SWEEP_FRAME_REGRESSION_CASES
        .iter()
        .copied()
        .chain(FORMAT_SWEEP_STRUCTURAL_REGRESSION_CASES.iter().copied())
        .chain(
            FORMAT_SWEEP_EXECUTABLE_BOUNDARY_REGRESSION_CASES
                .iter()
                .copied(),
        )
        .chain([
            (
                DatabaseType::Oracle,
                FORMAT_SWEEP_INLINE_QUERY_FRAME_REGRESSION,
            ),
            (
                DatabaseType::Oracle,
                FORMAT_SWEEP_INLINE_SELECT_COMMENT_REGRESSION,
            ),
            (DatabaseType::Oracle, FORMAT_SWEEP_WITH_FRAME_REGRESSION),
            (DatabaseType::MySQL, FORMAT_SWEEP_WITH_FRAME_REGRESSION),
            (DatabaseType::MariaDB, FORMAT_SWEEP_WITH_FRAME_REGRESSION),
        ])
        .chain(
            FORMAT_SWEEP_INLINE_CONTINUATION_REGRESSIONS
                .iter()
                .copied()
                .flat_map(|source| {
                    [
                        (DatabaseType::Oracle, source),
                        (DatabaseType::MySQL, source),
                        (DatabaseType::MariaDB, source),
                    ]
                }),
        );

    for (db_type, source) in built_in_regressions {
        checked_regressions = checked_regressions.saturating_add(1);
        let run = format_sweep_run(source, db_type);
        checked_identifier_case_words =
            checked_identifier_case_words.saturating_add(run.checked_identifier_case_words);
        checked_frames = checked_frames.saturating_add(run.checked_frames);
        checked_frame_boundaries =
            checked_frame_boundaries.saturating_add(run.checked_frame_boundaries);
        checked_frame_depth_symmetries =
            checked_frame_depth_symmetries.saturating_add(run.checked_frame_depth_symmetries);
        checked_frame_body_items =
            checked_frame_body_items.saturating_add(run.checked_frame_body_items);
        checked_frame_closes = checked_frame_closes.saturating_add(run.checked_frame_closes);
        for kind in &run.managed_frame_kinds {
            if !managed_frame_kinds.contains(kind) {
                managed_frame_kinds.push(*kind);
            }
        }
        for kind in &run.managed_list_owner_kinds {
            if !managed_list_owner_kinds.contains(kind) {
                managed_list_owner_kinds.push(*kind);
            }
        }
        if !run.issues.is_empty() {
            failures.push(format!(
                "built-in expanded-frame regression db={db_type:?} issues={}",
                run.issues.len()
            ));
        }
    }

    for (dir, db_type) in [
        ("test", DatabaseType::Oracle),
        ("test_mysql", DatabaseType::MySQL),
        ("test_mariadb", DatabaseType::MariaDB),
    ] {
        let input_dir = manifest.join(dir);
        let mut entries: Vec<PathBuf> = fs::read_dir(&input_dir)
            .unwrap_or_else(|err| panic!("failed to read `{}`: {err}", input_dir.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "sql" | "txt"))
            })
            .collect();
        entries.sort();

        for input_path in entries {
            checked_files += 1;
            let relative = input_path.strip_prefix(&manifest).unwrap_or(&input_path);
            let file_name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("format.sql");
            let out_path = output_root
                .join(relative.parent().unwrap_or(Path::new("")))
                .join(format!("{file_name}.format.out"));
            let run = format_sweep_generate_report_for_file(&input_path, db_type, &out_path, false);
            checked_identifier_case_words =
                checked_identifier_case_words.saturating_add(run.checked_identifier_case_words);
            checked_frames = checked_frames.saturating_add(run.checked_frames);
            checked_frame_boundaries =
                checked_frame_boundaries.saturating_add(run.checked_frame_boundaries);
            checked_frame_depth_symmetries =
                checked_frame_depth_symmetries.saturating_add(run.checked_frame_depth_symmetries);
            checked_frame_body_items =
                checked_frame_body_items.saturating_add(run.checked_frame_body_items);
            checked_frame_closes = checked_frame_closes.saturating_add(run.checked_frame_closes);
            for kind in &run.managed_frame_kinds {
                if !managed_frame_kinds.contains(kind) {
                    managed_frame_kinds.push(*kind);
                }
            }
            for kind in &run.managed_list_owner_kinds {
                if !managed_list_owner_kinds.contains(kind) {
                    managed_list_owner_kinds.push(*kind);
                }
            }
            if !run.issues.is_empty() {
                failures.push(format!(
                    "{} issues={} report={}",
                    relative.display(),
                    run.issues.len(),
                    out_path.display()
                ));
            }
        }
    }

    managed_frame_kinds.sort_unstable();
    managed_list_owner_kinds.sort_unstable();
    let mut aggregate = format!(
        "Auto-format sweep aggregate\nchecked_files={checked_files}\nchecked_regressions={checked_regressions}\nchecked_identifier_case_words={checked_identifier_case_words}\nchecked_frames={checked_frames}\nchecked_frame_boundaries={checked_frame_boundaries}\nchecked_frame_depth_symmetries={checked_frame_depth_symmetries}\nchecked_frame_body_items={checked_frame_body_items}\nchecked_frame_closes={checked_frame_closes}\nmanaged_frame_kinds={managed_frame_kinds:?}\nmanaged_list_owner_kinds={managed_list_owner_kinds:?}\nfailures={}\n",
        failures.len()
    );
    for failure in &failures {
        aggregate.push_str(failure);
        aggregate.push('\n');
    }
    fs::create_dir_all(&output_root).expect("create format sweep output directory");
    fs::write(output_root.join("format-sweep.out"), aggregate)
        .expect("write format sweep aggregate");
    assert!(
        failures.is_empty(),
        "auto-format sweep found {} failures across {} files and {} built-in regressions; see `{}`",
        failures.len(),
        checked_files,
        checked_regressions,
        output_root.join("format-sweep.out").display()
    );
    let mut expected_frame_kinds = FormatManagedFrameKind::ALL.to_vec();
    expected_frame_kinds.sort_unstable();
    assert_eq!(
        managed_frame_kinds, expected_frame_kinds,
        "the complete fixture sweep must exercise every production frame kind"
    );
}

#[test]
fn formatting_sweep_detects_identifier_case_change_next_to_dot() {
    let source = "SELECT r.name FROM qt_case_demo r;";
    let mutated = "SELECT r.NAME FROM qt_case_demo r;";
    let (checked, issues) =
        format_sweep_audit_identifier_case(source, mutated, DatabaseType::Oracle);
    assert!(checked > 0, "audit should pair word tokens");
    assert_eq!(
        issues.len(),
        1,
        "dot-qualified identifier case change must be reported, got {issues:?}"
    );
    assert!(matches!(
        issues[0].kind,
        FormatSweepIssueKind::IdentifierCase
    ));
}

#[test]
fn formatting_sweep_detects_identifier_case_change_after_goto_and_procedure() {
    let source = "BEGIN\n<<top>>\nNULL;\nGOTO top;\nEND;";
    let mutated = "BEGIN\n<<top>>\nNULL;\nGOTO TOP;\nEND;";
    let (_, issues) = format_sweep_audit_identifier_case(source, mutated, DatabaseType::Oracle);
    assert_eq!(
        issues.len(),
        1,
        "GOTO label case change must be reported, got {issues:?}"
    );

    let source =
        "CREATE OR REPLACE PACKAGE qt_pkg AS\n    PROCEDURE seed (p_rows IN NUMBER);\nEND qt_pkg;";
    let mutated =
        "CREATE OR REPLACE PACKAGE qt_pkg AS\n    PROCEDURE SEED (p_rows IN NUMBER);\nEND qt_pkg;";
    let (_, issues) = format_sweep_audit_identifier_case(source, mutated, DatabaseType::Oracle);
    assert_eq!(
        issues.len(),
        1,
        "PROCEDURE name case change must be reported, got {issues:?}"
    );
}

#[test]
fn formatting_sweep_accepts_keyword_case_normalization_outside_identifier_slots() {
    let source = "select 1 from qt_case_demo;";
    let formatted = "SELECT 1\nFROM qt_case_demo;";
    let (_, issues) = format_sweep_audit_identifier_case(source, formatted, DatabaseType::Oracle);
    assert!(
        issues.is_empty(),
        "keyword normalization away from identifier slots must pass, got {issues:?}"
    );
}

#[test]
fn inline_case_call_argument_keeps_paren_frame_for_following_arguments() {
    let source = "BEGIN\nqt_split_pkg.complex_upsert (p_user_id => v_ids (i), p_status => CASE WHEN MOD (v_ids (i), 2) = 0 THEN 'A' ELSE 'I' END,\np_note => 'generated');\nEND;";
    let formatted = SqlEditorWidget::format_sql_basic(source);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent_of = |needle: &str| {
        lines
            .iter()
            .find(|line| line.trim_start().starts_with(needle))
            .map(|line| line.len() - line.trim_start().len())
            .unwrap_or_else(|| panic!("line starting with {needle:?} not found in:\n{formatted}"))
    };
    let call_indent = indent_of("qt_split_pkg.complex_upsert");
    let end_indent = indent_of("END,");
    let next_arg_indent = indent_of("p_note =>");
    let when_indent = indent_of("WHEN MOD");
    assert_eq!(
        end_indent,
        call_indent + FORMAT_SWEEP_INDENT_WIDTH,
        "inline CASE END inside a call paren must sit one level inside the call frame, got:\n{formatted}"
    );
    assert_eq!(
        next_arg_indent, end_indent,
        "argument following the inline CASE must stay at the paren frame level, got:\n{formatted}"
    );
    assert_eq!(
        when_indent,
        end_indent + FORMAT_SWEEP_INDENT_WIDTH,
        "CASE branches must sit one level deeper than the CASE frame, got:\n{formatted}"
    );
}

#[test]
fn mysql_declare_condition_for_clause_stays_inline() {
    let source = "DECLARE user_error CONDITION FOR SQLSTATE '45000';";
    let formatted =
        SqlEditorWidget::format_sql_basic_no_cache_for_db_type(source, DatabaseType::MariaDB);
    assert!(
        formatted.contains("user_error CONDITION FOR SQLSTATE '45000';"),
        "DECLARE ... CONDITION FOR must stay on one line, got:\n{formatted}"
    );
}

#[test]
fn fetch_first_comment_continuation_stays_one_deeper_after_match_recognize_close() {
    let source = "CREATE OR REPLACE PROCEDURE p IS r SYS_REFCURSOR; BEGIN OPEN r FOR SELECT * FROM e MATCH_RECOGNIZE (PARTITION BY d ORDER BY rn MEASURES FIRST (n) AS s ONE ROW PER MATCH PATTERN (A B+) DEFINE B AS B.sal > PREV (B.sal)) FETCH FIRST /* BV */ 20 ROWS ONLY; END p;";
    let formatted = SqlEditorWidget::format_sql_basic(source);
    let lines: Vec<&str> = formatted.lines().collect();
    let fetch_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("FETCH FIRST"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("FETCH FIRST line");
    let operand_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("20 ROWS ONLY"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("FETCH operand line");
    assert_eq!(
        operand_indent,
        fetch_indent + FORMAT_SWEEP_INDENT_WIDTH,
        "comment-split FETCH operand must stay one level deeper than the FETCH line, got:\n{formatted}"
    );
}

#[test]
fn match_recognize_define_condition_continuation_anchors_to_item_line() {
    let source = "CREATE OR REPLACE PROCEDURE p IS r SYS_REFCURSOR; BEGIN OPEN r FOR SELECT * FROM e MATCH_RECOGNIZE (PARTITION BY d ORDER BY rn MEASURES FIRST (n) AS s ONE ROW PER MATCH PATTERN (A B+) DEFINE\n-- c\nB AS B.sal > PREV (B.sal) AND B.sal < 10) FETCH FIRST 20 ROWS ONLY; END p;";
    let formatted = SqlEditorWidget::format_sql_basic(source);
    let lines: Vec<&str> = formatted.lines().collect();
    let item_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("B AS B.sal > PREV"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("DEFINE item line");
    let and_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("AND B.sal"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("DEFINE condition continuation line");
    assert_eq!(
        and_indent,
        item_indent + FORMAT_SWEEP_INDENT_WIDTH,
        "DEFINE's later condition sibling must use the condition-frame depth, got:\n{formatted}"
    );
}

#[test]
fn adversarial_frame_probes_stay_idempotent() {
    let probes: Vec<(&str, &str, DatabaseType)> = vec![
        (
            "P1 nested call inline CASE arg",
            "BEGIN\nouter_call (a => inner_call (x => 1, y => CASE WHEN v = 1 THEN 'p' ELSE 'q' END,\nz => 2), b => CASE WHEN w = 2 THEN 'r' ELSE 's' END,\nc => 3);\nEND;",
            DatabaseType::Oracle,
        ),
        (
            "P2 inline CASE arg deep in package body IF",
            "CREATE OR REPLACE PACKAGE BODY pkg AS\nPROCEDURE p IS\nBEGIN\nIF x = 1 THEN\nFOR i IN 1..3 LOOP\nlog_call (p_tag => 'T', p_val => CASE WHEN MOD (i, 2) = 0 THEN 'E' ELSE 'O' END,\np_extra => i);\nEND LOOP;\nEND IF;\nEND p;\nEND pkg;\n/",
            DatabaseType::Oracle,
        ),
        (
            "P3 CASE as first arg after open paren",
            "BEGIN\nfoo (CASE WHEN a = 1 THEN 'x' ELSE 'y' END,\n2, 3);\nEND;",
            DatabaseType::Oracle,
        ),
        (
            "P4 SQL select-list call with => CASE",
            "SELECT pkg.fn (p_a => CASE WHEN t.c = 1 THEN 'a' ELSE 'b' END,\np_b => 2) AS v FROM t;",
            DatabaseType::Oracle,
        ),
        (
            "P5 MariaDB CONDITION FOR error code nested",
            "CREATE PROCEDURE sp()\nBEGIN\nDECLARE dup_key CONDITION FOR 1062;\nDECLARE CONTINUE HANDLER FOR dup_key\nBEGIN\nSET @x = 1;\nEND;\nBEGIN\nDECLARE bad_state CONDITION FOR SQLSTATE '45000';\nSET @y = 2;\nEND;\nEND",
            DatabaseType::MariaDB,
        ),
        (
            "P6 FETCH comment continuation after PIVOT close",
            "SELECT * FROM (SELECT deptno, job, sal FROM emp) PIVOT (SUM (sal) FOR job IN ('A' AS a, 'B' AS b)) ORDER BY deptno FETCH FIRST /* top */ 5 ROWS ONLY;",
            DatabaseType::Oracle,
        ),
        (
            "P7 FETCH comment continuation nested in OPEN FOR + UNPIVOT",
            "CREATE OR REPLACE PROCEDURE p IS r SYS_REFCURSOR; BEGIN OPEN r FOR SELECT * FROM (SELECT a, b FROM t) UNPIVOT (v FOR k IN (a AS 'A', b AS 'B')) ORDER BY k FETCH FIRST /* top */ 3 ROWS ONLY; END p;",
            DatabaseType::Oracle,
        ),
        (
            "P8 MR DEFINE multi-item with comment and AND",
            "SELECT * FROM e MATCH_RECOGNIZE (PARTITION BY d ORDER BY rn PATTERN (L+ H) DEFINE\n-- low band\nL AS L.v < 10 AND L.w > 2,\n-- high band\nH AS H.v > 50 AND H.w < 9);",
            DatabaseType::Oracle,
        ),
        (
            "P9 MR DEFINE with subquery WHERE AND inside",
            "SELECT * FROM e MATCH_RECOGNIZE (PARTITION BY d ORDER BY rn PATTERN (A B+) DEFINE B AS B.sal > (SELECT AVG (x.sal) FROM e x WHERE x.d = B.d AND x.flag = 'Y'));",
            DatabaseType::Oracle,
        ),
        (
            "P10 MR inside MERGE USING subquery with DEFINE AND",
            "MERGE INTO tgt t USING (SELECT * FROM e MATCH_RECOGNIZE (PARTITION BY d ORDER BY rn PATTERN (A B+) DEFINE\n-- cond\nB AS B.sal > PREV (B.sal) AND B.sal < 99)) s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.v = s.v;",
            DatabaseType::Oracle,
        ),
    ];
    let mut failures = Vec::new();
    for (name, src, db) in probes {
        let once = SqlEditorWidget::format_sql_basic_no_cache_for_db_type(src, db);
        let twice = SqlEditorWidget::format_sql_basic_no_cache_for_db_type(&once, db);
        let idempotent = once == twice;
        println!("===== {name} (idempotent={idempotent}) =====\n{once}\n");
        if !idempotent {
            println!("----- SECOND PASS DIFFERS -----\n{twice}\n");
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "non-idempotent probes: {failures:?}");
}
