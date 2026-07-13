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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FormatSweepIssueKind {
    FormatPanic,
    ItemOrTokenChanged,
    Indentation,
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
    let source = SqlEditorWidget::restore_inline_sqlplus_command_boundaries(source);
    let source_fingerprint = format_sweep_statement_fingerprint(&source, db_type);
    let formatted_fingerprint = format_sweep_statement_fingerprint(formatted, db_type);
    (source_fingerprint != formatted_fingerprint).then(|| {
        let first_mismatch = source_fingerprint
            .iter()
            .zip(formatted_fingerprint.iter())
            .position(|(left, right)| left != right)
            .unwrap_or_else(|| source_fingerprint.len().min(formatted_fingerprint.len()));
        let source_preview = source_fingerprint
            .get(first_mismatch)
            .map(|tokens| tokens.iter().take(6).cloned().collect::<Vec<_>>());
        let formatted_preview = formatted_fingerprint
            .get(first_mismatch)
            .map(|tokens| tokens.iter().take(6).cloned().collect::<Vec<_>>());
        FormatSweepIssue::new(
            FormatSweepIssueKind::ItemOrTokenChanged,
            formatted,
            0,
            format!(
                "first formatting pass changed SQL statement items or tokens at item {}; item counts {} -> {}; source={source_preview:?} formatted={formatted_preview:?}",
                first_mismatch + 1,
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
        SqlEditorWidget::format_for_auto_formatting_with_db_type(source, false, Some(db_type))
    }));
    let formatted = match first {
        Ok(formatted) => formatted,
        Err(payload) => {
            return FormatSweepRun {
                formatted: source.to_string(),
                checked_lines: 0,
                checked_gaps: 0,
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
        "-- checked: lines={} token_gaps={} probes={}\n",
        run.checked_lines, run.checked_gaps, run.probes
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
    assert_eq!(run.probes, 4, "three whitespace probes plus idempotence");
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

    let mut aggregate = format!(
        "Auto-format sweep aggregate\nchecked_files={checked_files}\nfailed_files={}\n",
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
        "auto-format sweep found issues in {} of {} files; see `{}`",
        failures.len(),
        checked_files,
        output_root.join("format-sweep.out").display()
    );
}
