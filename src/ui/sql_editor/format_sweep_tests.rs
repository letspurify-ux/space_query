use super::formatter::{FormatManagedFrameKind, ListOwnerKind};
use super::{query_text, SqlEditorWidget, SqlToken};
use crate::db::connection::DatabaseType;
use crate::db::{FormatItem, QueryExecutor, ScriptItem};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FormatSweepIssueKind {
    FormatPanic,
    ItemOrTokenChanged,
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
    checked_frames: usize,
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

#[derive(Clone, Copy)]
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

fn format_sweep_tokens(statement: &str, db_type: DatabaseType) -> Vec<FormatSweepToken> {
    query_text::tokenize_sql_spanned_with_mysql_compat(statement, mysql_compatible(db_type))
        .into_iter()
        .map(|span| match span.token {
            SqlToken::Word(word) => FormatSweepToken::Word(word.to_ascii_uppercase()),
            SqlToken::String(value) => FormatSweepToken::String(value),
            SqlToken::Comment(value) => FormatSweepToken::Comment(value.trim_end().to_string()),
            SqlToken::Symbol(value) => FormatSweepToken::Symbol(value),
        })
        .collect()
}

fn format_sweep_statement_fingerprint(
    text: &str,
    db_type: DatabaseType,
) -> Vec<Vec<FormatSweepToken>> {
    QueryExecutor::split_script_items_for_db_type(text, Some(db_type))
        .into_iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(statement) => {
                let tokens = format_sweep_tokens(&statement, db_type);
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

fn protected_payload_lines(text: &str, db_type: DatabaseType, line_count: usize) -> Vec<bool> {
    let mut protected = vec![false; line_count];
    let line_starts = line_start_offsets(text);
    let spans = query_text::tokenize_sql_spanned_with_mysql_compat(text, mysql_compatible(db_type));
    for span in spans {
        if !token_is_comment_or_string(&span.token)
            || !text
                .get(span.start..span.end)
                .is_some_and(|token| token.contains('\n'))
        {
            continue;
        }
        let start_line = line_starts
            .partition_point(|start| *start <= span.start)
            .saturating_sub(1);
        let end_line = line_starts
            .partition_point(|start| *start < span.end)
            .saturating_sub(1);
        for line in start_line..=end_line.min(protected.len().saturating_sub(1)) {
            protected[line] = true;
        }
    }
    protected
}

fn format_sweep_audit_first_pass(
    formatted: &str,
    db_type: DatabaseType,
) -> (usize, usize, Vec<FormatSweepIssue>) {
    let mut issues = Vec::new();
    let lines: Vec<&str> = formatted.lines().collect();
    let line_starts = line_start_offsets(formatted);
    let protected_lines = protected_payload_lines(formatted, db_type, lines.len());
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

    (
        checked_lines,
        format_sweep_safe_gap_count(formatted, db_type),
        issues,
    )
}

fn format_sweep_audit_token_count(
    source: &str,
    formatted: &str,
    db_type: DatabaseType,
) -> Option<FormatSweepIssue> {
    let source_fingerprint = format_sweep_statement_fingerprint(source, db_type);
    let formatted_fingerprint = format_sweep_statement_fingerprint(formatted, db_type);
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

fn token_is_comment_or_string(token: &SqlToken) -> bool {
    matches!(token, SqlToken::Comment(_) | SqlToken::String(_))
}

fn token_label(token: &SqlToken) -> &str {
    match token {
        SqlToken::Word(word)
        | SqlToken::String(word)
        | SqlToken::Comment(word)
        | SqlToken::Symbol(word) => word,
    }
}

fn format_sweep_safe_gap_count(text: &str, db_type: DatabaseType) -> usize {
    let spans = query_text::tokenize_sql_spanned_with_mysql_compat(text, mysql_compatible(db_type));
    spans
        .windows(2)
        .filter(|pair| {
            let [left, right] = pair else {
                return false;
            };
            !token_is_comment_or_string(&left.token)
                && !token_is_comment_or_string(&right.token)
                && text.get(left.end..right.start).is_some_and(|gap| {
                    !gap.is_empty() && gap.bytes().all(|byte| byte.is_ascii_whitespace())
                })
        })
        .count()
}

fn mutate_format_gaps(text: &str, db_type: DatabaseType, kind: FormatSweepProbeKind) -> String {
    let spans = query_text::tokenize_sql_spanned_with_mysql_compat(text, mysql_compatible(db_type));
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len().saturating_add(64));
    let mut cursor = 0usize;
    for (idx, span) in spans.iter().enumerate() {
        let gap = text.get(cursor..span.start).unwrap_or_default();
        let previous = idx.checked_sub(1).and_then(|prev| spans.get(prev));
        let safe = !gap.is_empty()
            && gap.bytes().all(|byte| byte.is_ascii_whitespace())
            && previous.is_none_or(|prev| !token_is_comment_or_string(&prev.token))
            && !token_is_comment_or_string(&span.token);
        if safe {
            match kind {
                FormatSweepProbeKind::Reindent if gap.contains('\n') => {
                    let newline_count = gap.bytes().filter(|byte| *byte == b'\n').count();
                    for _ in 0..newline_count {
                        out.push('\n');
                    }
                    out.push_str("       ");
                }
                FormatSweepProbeKind::CollapseBreaks if gap.contains('\n') => out.push(' '),
                FormatSweepProbeKind::ExpandInline if !gap.contains('\n') && idx % 3 == 0 => {
                    out.push_str("\n       ")
                }
                _ => out.push_str(gap),
            }
        } else {
            out.push_str(gap);
        }
        out.push_str(text.get(span.start..span.end).unwrap_or_default());
        cursor = span.end;
    }
    out.push_str(text.get(cursor..).unwrap_or_default());
    out
}

fn format_probe_text(
    formatted: &str,
    db_type: DatabaseType,
    kind: FormatSweepProbeKind,
) -> Option<String> {
    let items = query_text::split_format_items_for_db_type(formatted, Some(db_type));
    let mut probe = String::with_capacity(formatted.len().saturating_add(128));
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            probe.push_str("\n\n");
        }
        match item {
            FormatItem::Statement(statement) => {
                probe.push_str(&mutate_format_gaps(statement, db_type, kind));
            }
            FormatItem::ToolCommand(command) => {
                probe.push_str(&SqlEditorWidget::format_tool_command(command));
            }
            FormatItem::Verbatim(value) => probe.push_str(value),
            FormatItem::Slash => probe.push('/'),
        }
    }
    (format_sweep_statement_fingerprint(&probe, db_type)
        == format_sweep_statement_fingerprint(formatted, db_type))
    .then_some(probe)
}

fn whitespace_kind_between_equal_tokens(
    baseline: &str,
    comparison: &str,
    db_type: DatabaseType,
) -> (FormatSweepIssueKind, usize, String) {
    let left =
        query_text::tokenize_sql_spanned_with_mysql_compat(baseline, mysql_compatible(db_type));
    let right =
        query_text::tokenize_sql_spanned_with_mysql_compat(comparison, mysql_compatible(db_type));
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
        left_cursor = left_span.end;
        right_cursor = right_span.end;
    }
    (
        FormatSweepIssueKind::WhitespaceDependency,
        baseline.len(),
        "formatted outputs differ after the final token".to_string(),
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
                checked_frames: 0,
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
    let mut probes = 0usize;
    for kind in [
        FormatSweepProbeKind::Reindent,
        FormatSweepProbeKind::CollapseBreaks,
        FormatSweepProbeKind::ExpandInline,
    ] {
        let Some(probe) = format_probe_text(&formatted, db_type, kind) else {
            continue;
        };
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
                    format!("{layout_kind:?}: {message}"),
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
        checked_frames: frame_alignment_audit.checked_frames,
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
        "-- checked: lines={} token_gaps={} frames={} frame_body_items={} frame_closes={} probes={}\n",
        run.checked_lines,
        run.checked_gaps,
        run.checked_frames,
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
fn formatting_sweep_first_pass_detects_token_loss() {
    let source = "SELECT a, b FROM t;";
    let formatted_with_loss = "SELECT a FROM t;";
    let issue = format_sweep_audit_token_count(source, formatted_with_loss, DatabaseType::Oracle)
        .expect("token loss should be reported on the first pass");
    assert_eq!(issue.kind, FormatSweepIssueKind::ItemOrTokenChanged);
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
    let run = format_sweep_run(
        "SELECT a, b FROM t WHERE a = 1 AND b = 2;",
        DatabaseType::Oracle,
    );
    assert!(run.issues.is_empty(), "unexpected issues: {:?}", run.issues);
    assert!(
        run.formatted.contains("WHERE\n    a = 1\n    AND b = 2"),
        "multiline condition children should share one depth:\n{}",
        run.formatted
    );
    assert_eq!(run.probes, 4, "three whitespace probes plus idempotence");
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
fn formatting_sweep_expanded_frames_start_first_child_at_final_depth() {
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
            .position(|line| line.trim() == "WHEN")
            .unwrap_or_else(|| panic!("WHEN owner not found:\n{}", run.formatted));
        let first_child = lines
            .get(when_idx + 1)
            .copied()
            .unwrap_or_else(|| panic!("WHEN first child not found:\n{}", run.formatted));
        let nested_select = lines
            .get(when_idx + 2)
            .copied()
            .unwrap_or_else(|| panic!("nested SELECT not found:\n{}", run.formatted));
        let first_close = lines
            .iter()
            .skip(when_idx + 3)
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
            indent(first_child),
            indent(lines[when_idx]) + FORMAT_SWEEP_INDENT_WIDTH,
            "first condition child must start at owner + 1:\n{}",
            run.formatted
        );
        assert_eq!(
            indent(nested_select),
            indent(first_child) + FORMAT_SWEEP_INDENT_WIDTH,
            "nested frame body must start at its owner + 1:\n{}",
            run.formatted
        );
        assert_eq!(
            indent(first_close),
            indent(first_child),
            "nested frame start and end must share one depth:\n{}",
            run.formatted
        );
        assert_eq!(
            indent(and_child),
            indent(first_child),
            "condition siblings must share one depth:\n{}",
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
            .contains("SEARCH DEPTH FIRST BY id,\n    parent_id SET traversal_no"),
        "SEARCH siblings should use one list-body depth:\n{}",
        run.formatted
    );
    assert!(
        run.formatted
            .contains("CYCLE id,\n    parent_id SET cycle_yn"),
        "CYCLE siblings should use one list-body depth:\n{}",
        run.formatted
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
            "ALTER TABLE t ADD a NUMBER, ADD b NUMBER; LOCK TABLE t_a, t_b IN EXCLUSIVE MODE; FLASHBACK TABLE t_a, t_b TO SCN 1; CREATE OR REPLACE TRIGGER trg_order BEFORE INSERT ON t FOLLOWS trg_a, trg_b BEGIN NULL; END;",
        ),
        (
            DatabaseType::Oracle,
            "WITH r (id, parent_id) AS (SELECT 1, 0 FROM DUAL UNION ALL SELECT id + 1, id FROM r WHERE id < 3) SEARCH DEPTH FIRST BY id, parent_id SET traversal_no CYCLE id, parent_id SET cycle_yn TO 'Y' DEFAULT 'N' SELECT id, parent_id FROM r;",
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
            run.formatted.starts_with("WITH\n    first_cte AS ("),
            "{db_type:?} first CTE should start at WITH owner+1 depth:\n{}",
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
        checked_frames: 0,
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
    let mut checked_frames = 0usize;
    let mut checked_frame_body_items = 0usize;
    let mut checked_frame_closes = 0usize;
    let mut managed_frame_kinds = Vec::new();
    let mut managed_list_owner_kinds = Vec::new();

    let built_in_regressions = FORMAT_SWEEP_FRAME_REGRESSION_CASES
        .iter()
        .copied()
        .chain(FORMAT_SWEEP_STRUCTURAL_REGRESSION_CASES.iter().copied())
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
        checked_frames = checked_frames.saturating_add(run.checked_frames);
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
            checked_frames = checked_frames.saturating_add(run.checked_frames);
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
        "Auto-format sweep aggregate\nchecked_files={checked_files}\nchecked_regressions={checked_regressions}\nchecked_frames={checked_frames}\nchecked_frame_body_items={checked_frame_body_items}\nchecked_frame_closes={checked_frame_closes}\nmanaged_frame_kinds={managed_frame_kinds:?}\nmanaged_list_owner_kinds={managed_list_owner_kinds:?}\nfailures={}\n",
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
        .find(|line| line.trim_start() == "B AS")
        .map(|line| line.len() - line.trim_start().len())
        .expect("DEFINE item line");
    let first_condition_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("B.sal > PREV"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("DEFINE first condition line");
    let and_indent = lines
        .iter()
        .find(|line| line.trim_start().starts_with("AND B.sal"))
        .map(|line| line.len() - line.trim_start().len())
        .expect("DEFINE condition continuation line");
    assert_eq!(
        first_condition_indent,
        item_indent + FORMAT_SWEEP_INDENT_WIDTH,
        "DEFINE first condition must sit one level deeper than its item line, got:\n{formatted}"
    );
    assert_eq!(
        and_indent, first_condition_indent,
        "DEFINE condition siblings must share one depth, got:\n{formatted}"
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
