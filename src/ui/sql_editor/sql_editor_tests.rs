use super::*;
use crate::db::{FormatItem, QueryExecutor, ScriptItem};
use crate::ui::syntax_highlight::{
    encode_fltk_style_bytes, STYLE_COMMENT, STYLE_DEFAULT, STYLE_KEYWORD, STYLE_STRING,
};

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn load_test_file(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("test");
    path.push(name);
    fs::read_to_string(path).unwrap_or_default()
}

fn load_mariadb_test_file(name: &str) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for dir in ["test_mysql", "test_mariadb"] {
        let path = manifest_dir.join(dir).join(name);
        if let Ok(contents) = fs::read_to_string(path) {
            return contents;
        }
    }
    String::new()
}

fn count_slash_lines(text: &str) -> usize {
    text.lines().filter(|line| line.trim() == "/").count()
}

fn count_script_statements(items: &[ScriptItem]) -> usize {
    items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count()
}

fn count_script_tool_commands(items: &[ScriptItem]) -> usize {
    items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count()
}

fn assert_contains_all(haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "Expected output to contain: {}",
            needle
        );
    }
}

fn leading_spaces(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn find_line_starting_with(lines: &[&str], prefix: &str) -> Option<usize> {
    lines
        .iter()
        .position(|line| line.trim_start().starts_with(prefix))
}

fn apply_incremental_highlight_for_test(
    original_text: &str,
    updated_text: &str,
    pos: usize,
    inserted_len: usize,
    deleted_len: usize,
) -> Option<String> {
    let highlighter = SqlHighlighter::new();
    let (previous_styles, previous_line_exit_states) =
        build_logical_styles_and_line_states(&highlighter, original_text);
    let mut shadow = HighlightShadowState::default();
    shadow.rebuild(
        original_text.to_string(),
        &previous_styles,
        previous_line_exit_states,
    );

    let inserted_end = pos.saturating_add(inserted_len).min(updated_text.len());
    let inserted_text = updated_text.get(pos.min(updated_text.len())..inserted_end)?;
    if !shadow.apply_edit(pos, inserted_text, deleted_len) {
        return None;
    }

    let text_len = shadow.len();
    let start = incremental_rehighlight_start_for_text(updated_text, pos).min(text_len);
    let must_cover_end =
        incremental_direct_rehighlight_end_for_text(updated_text, pos, inserted_len, deleted_len);
    let mut current_line_idx = shadow.line_index_for_position(start);
    let mut entry_state = shadow.entry_state_for_line(current_line_idx);

    while current_line_idx < shadow.line_count() {
        let current_start = shadow.line_start_for_index(current_line_idx);
        let current_end = shadow.inclusive_line_end_for_index(current_line_idx);
        let range_text = shadow.text.get(current_start..current_end)?;
        let previous_styles = shadow.styles.get(current_start..current_end)?;
        let old_exit_state = shadow.line_exit_state(current_line_idx);
        let (new_styles, new_exit_state) =
            highlighter.generate_styles_for_window(range_text, entry_state);
        if new_styles.len() != range_text.len() {
            return None;
        }

        let styles_changed = new_styles.as_bytes() != previous_styles;
        if styles_changed {
            let style_slice = shadow.styles.get_mut(current_start..current_end)?;
            style_slice.copy_from_slice(new_styles.as_bytes());
        }
        shadow.set_line_exit_state(current_line_idx, new_exit_state);

        if current_end >= must_cover_end
            && !styles_changed
            && old_exit_state == Some(new_exit_state)
        {
            break;
        }

        current_line_idx = current_line_idx.saturating_add(1);
        entry_state = new_exit_state;
    }

    std::str::from_utf8(&shadow.styles)
        .ok()
        .map(ToString::to_string)
}

fn line_start_for_text(text: &str, pos: usize) -> usize {
    let clamped = pos.min(text.len());
    let mut boundary = clamped;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.get(..boundary)
        .and_then(|prefix| prefix.rfind('\n'))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

fn inclusive_line_end_for_text(text: &str, pos: usize) -> usize {
    let clamped = pos.min(text.len());
    let mut boundary = clamped;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let bytes = text.as_bytes();
    let mut idx = boundary;
    while idx < bytes.len() {
        match bytes.get(idx).copied() {
            Some(b'\n') => return idx.saturating_add(1),
            Some(b'\r') => {
                if bytes.get(idx.saturating_add(1)) == Some(&b'\n') {
                    return idx.saturating_add(2);
                }
                return idx.saturating_add(1);
            }
            Some(_) => idx += 1,
            None => break,
        }
    }
    text.len()
}

fn incremental_rehighlight_start_for_text(text: &str, pos: usize) -> usize {
    line_start_for_text(text, pos)
}

fn incremental_direct_rehighlight_end_for_text(
    text: &str,
    pos: usize,
    inserted_len: usize,
    deleted_len: usize,
) -> usize {
    if text.is_empty() {
        return 0;
    }
    let changed_end = pos
        .saturating_add(inserted_len.max(deleted_len))
        .min(text.len());
    inclusive_line_end_for_text(text, changed_end)
}

#[test]
fn update_alert_pump_state_after_display_reschedules_when_queue_not_empty() {
    let mut pump_scheduled = true;
    let should_schedule = update_alert_pump_state_after_display(false, &mut pump_scheduled);
    assert!(should_schedule);
    assert!(pump_scheduled);

    let should_schedule_empty = update_alert_pump_state_after_display(true, &mut pump_scheduled);
    assert!(!should_schedule_empty);
    assert!(!pump_scheduled);
}

#[test]
fn column_poll_pending_action_state_machine_transitions_and_clear_rules() {
    let mut action = ColumnPollPendingAction::None;
    action.request_refresh();
    assert_eq!(action, ColumnPollPendingAction::Refresh);
    assert!(action.should_refresh());
    assert!(!action.should_clear(false));

    let mut action = ColumnPollPendingAction::None;
    action.request_clear();
    assert_eq!(action, ColumnPollPendingAction::Clear);
    assert!(!action.should_refresh());
    assert!(action.should_clear(false));
    assert!(!action.should_clear(true));

    let mut action = ColumnPollPendingAction::Refresh;
    action.request_clear();
    assert_eq!(action, ColumnPollPendingAction::RefreshThenClear);
    assert!(action.should_refresh());
    assert!(action.should_clear(false));
    assert!(!action.should_clear(true));
}

#[test]
fn is_window_shown_and_visible_requires_both_flags() {
    assert!(is_window_shown_and_visible(true, true));
    assert!(!is_window_shown_and_visible(true, false));
    assert!(!is_window_shown_and_visible(false, true));
    assert!(!is_window_shown_and_visible(false, false));
}

#[test]
fn default_style_text_for_len_matches_requested_length() {
    let styles = SqlEditorWidget::default_style_text_for_len(8);
    assert_eq!(styles.chars().count(), 8);
    assert!(styles.chars().all(|ch| ch == STYLE_DEFAULT));
}

#[test]
fn sql_editor_alert_calls_use_wrapper_function() {
    let mod_src = include_str!("mod.rs");
    assert!(
        mod_src.contains("pub(crate) fn show_alert_dialog"),
        "SqlEditorWidget::show_alert_dialog helper must be defined in mod.rs"
    );
    assert!(
        mod_src.contains("struct PendingAlertState"),
        "mod.rs should keep a single shared alert queue state"
    );
    assert!(
        mod_src.contains("fn drain_pending_alerts()"),
        "mod.rs should process alerts through a single drain function"
    );
    assert!(
        mod_src.contains("is_window_shown_and_visible(window.shown(), window.visible())"),
        "main window visibility check should require shown() and visible()"
    );
    assert!(
        !mod_src.contains("fn show_alert_when_main_window_visible"),
        "legacy per-alert recursive retry helper should not remain"
    );
    assert_eq!(
        mod_src.matches("fltk::dialog::alert_default(").count(),
        1,
        "mod.rs should call fltk::dialog::alert_default only inside queue drain"
    );

    let file_checks = [("execution.rs", include_str!("execution.rs"))];

    for (name, source) in file_checks {
        assert_eq!(
            source.matches("fltk::dialog::alert_default(").count(),
            0,
            "{name} should route alerts through SqlEditorWidget::show_alert_dialog"
        );
    }
}

#[test]
fn format_sql_preserves_script_commands_and_slashes() {
    let cases = [
        (
            "test1.txt",
            vec![
                "Prompt 프로시저 테스트1",
                "SET SERVEROUTPUT ON",
                "SHOW ERRORS",
            ],
            vec![
                "OQT(Oracle Query Tool) - Procedure/Function Test Script",
                "-- 1) TEST DATA / TABLES",
            ],
            true,
        ),
        (
            "test2.txt",
            vec![
                "prompt 프로시저 테스트 4",
                "SET SERVEROUTPUT ON SIZE UNLIMITED",
                "SHOW ERRORS PACKAGE oqt_pkg",
                "SHOW ERRORS PACKAGE BODY oqt_pkg",
            ],
            vec![
                "PROMPT === [5] CALL VARIANTS: EXEC/BEGIN/DEFAULT/NAMED/POSITIONAL/NULL/UNICODE ===",
            ],
            true,
        ),
        (
            "test3.txt",
            vec![
                "Prompt 프로시저 테스트3",
                "SET DEFINE OFF",
                "PROMPT === [B] Cleanup ===",
                "SHOW ERRORS",
            ],
            vec![
                "OQT (Oracle Query Tool) Compatibility Test Script (TOAD-like)",
            ],
            false,
        ),
    ];

    for (file, expected_lines, comment_snippets, assert_idempotence) in cases {
        let input = load_test_file(file);
        let formatted = SqlEditorWidget::format_sql_basic(&input);

        assert_contains_all(&formatted, &expected_lines);
        assert_contains_all(&formatted, &comment_snippets);

        let input_slashes = count_slash_lines(&input);
        let output_slashes = count_slash_lines(&formatted);
        assert_eq!(
            input_slashes, output_slashes,
            "Slash terminator count differs for {}",
            file
        );

        if assert_idempotence {
            let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
            assert_eq!(
                formatted, formatted_again,
                "Formatting should be idempotent for {}",
                file
            );
        }
    }
}

#[test]
fn format_sql_preserves_connect_password_with_at_sign() {
    let input = "CONNECT user/p@ss@localhost:1521/ORCL\nSELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CONNECT user/p@ss@localhost:1521/ORCL"),
        "CONNECT password containing @ should be preserved, got:\n{}",
        formatted
    );
    assert!(
        formatted.contains("SELECT 1\nFROM DUAL;"),
        "SELECT statement should still be formatted normally, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_connect_password_with_slash() {
    let input = "CONNECT user/pa/ss@localhost:1521/ORCL
SELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CONNECT user/pa/ss@localhost:1521/ORCL"),
        "CONNECT password containing / should be preserved, got:
{}",
        formatted
    );
    assert!(
        formatted.contains(
            "SELECT 1
FROM DUAL;"
        ),
        "SELECT statement should still be formatted normally, got:
{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_connect_tns_alias() {
    let input = "CONNECT user/pass@LOCAL_FREE\nSELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CONNECT user/pass@LOCAL_FREE"),
        "CONNECT TNS alias should be preserved, got:\n{}",
        formatted
    );
    assert!(
        formatted.contains("SELECT 1\nFROM DUAL;"),
        "SELECT statement should still be formatted normally, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_select_hint_comment_is_idempotent() {
    let input = "SELECT /*+ INDEX(emp emp_idx1) */\nempno,\nename\nFROM emp;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let formatted_again = SqlEditorWidget::format_sql_basic_for_db_type(
        &formatted,
        crate::db::connection::DatabaseType::MySQL,
    );

    assert_eq!(
        formatted, formatted_again,
        "SELECT hint comment formatting should be idempotent"
    );
    assert!(
        formatted.contains("SELECT /*+ INDEX(emp emp_idx1) */"),
        "Expected optimizer hint comment to be preserved, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_cte_select_hint_keeps_following_columns_indented() {
    let input = r#"WITH sales_ranked AS (
SELECT
/*+ MATERIALIZE */
e.emp_id,
e.emp_name,
d.dept_name
FROM qt_emp e
JOIN qt_dept d
ON d.dept_id = e.dept_id
)
SELECT *
FROM sales_ranked;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("    SELECT /*+ MATERIALIZE */\n        e.emp_id,"),
        "CTE SELECT hint should preserve select-list indentation, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_mega_torture_script() {
    let input = load_test_file("mega_torture.txt");
    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        &input,
        crate::db::connection::DatabaseType::MySQL,
    );

    let expected_lines = vec![
        "PROMPT [0] bind/substitution setup",
        "WHENEVER SQLERROR EXIT SQL.SQLCODE",
        "SHOW ERRORS PACKAGE BODY oqt_mega_pkg",
        "PROMPT [6] trigger (extra nesting surface)",
        "PROMPT [DONE]",
    ];
    let comment_snippets = vec![
        "q'[ | tokens: END; / ; /* */ -- ]'",
        "q'[ |trg tokens: END; / ; /* */ -- ]'",
        "q'[ |q-quote: END; / ; /* */ -- ]'",
    ];

    assert_contains_all(&formatted, &expected_lines);
    assert_contains_all(&formatted, &comment_snippets);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for mega_torture.txt"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic_for_db_type(
        &formatted,
        crate::db::connection::DatabaseType::MySQL,
    );
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for mega_torture.txt"
    );
}

#[test]
fn format_sql_preserves_test15_nested_q_quote_script() {
    let input = load_test_file("test15.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);

    let expected_lines = vec![
        "CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg",
        "payload = q'[dynamic ; payload / still string]'",
        "END qt_splitter_pkg;",
        "CREATE OR REPLACE TRIGGER qt_splitter_biu",
        "WITH base_data AS",
    ];

    assert_contains_all(&formatted, &expected_lines);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for test15.sql"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test15.sql"
    );
}

#[test]
fn format_sql_preserves_test16_final_ultimate_boss_script() {
    let input = load_test_file("test16.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);

    let expected_lines = vec![
        "SET DEFINE ON",
        "SET SERVEROUTPUT ON",
        "PROMPT === QT SPLITTER FINAL ULTIMATE BOSS START ===",
        "CREATE OR REPLACE PROCEDURE qt_splitter_ultimate_proc",
        "AND t.\"COMMENT\" LIKE q'[%;%]'",
        "v_rendered := q'[fallback ; / ]'",
        "q'[payload from merge_like ; / ]'",
        "q'[dyn ; / -- '' ]'",
        "END qt_splitter_ultimate_proc;",
        "END qt_splitter_ultimate_pkg;",
        "PROMPT === QT SPLITTER FINAL ULTIMATE BOSS END ===",
    ];

    assert_contains_all(&formatted, &expected_lines);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for test16.sql"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test16.sql"
    );
}

#[test]
fn format_sql_preserves_test17_execution_unit_final_boss_script() {
    let input = load_test_file("test17.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);

    let expected_lines = vec![
        "CREATE OR REPLACE PACKAGE BODY qt_split_pkg",
        "q'{ | q2=/* not comment */ }'",
        "END qt_split_proc;",
        "END qt_split_trg;",
        "v_q1 := q'[",
        "SELECT unit_name,",
    ];

    assert_contains_all(&formatted, &expected_lines);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for test17.sql"
    );

    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test17.sql"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_split_pkg")
                && stmt.contains("q'{ | q2=/* not comment */ }'")
                && stmt.contains("END qt_split_pkg")
        }),
        "Formatting should preserve package body execution unit for test17.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_split_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_split_proc")
        }),
        "Formatting should preserve standalone procedure execution unit for test17.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("v_q1 := q'[")
                && stmt.contains("END lvl1;")
                && stmt.contains("END;")
        }),
        "Formatting should preserve lexical trap anonymous block for test17.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("SELECT log_id,")
                && stmt.contains("payload_preview")
                && stmt.contains("ORDER BY log_id")
        }),
        "Formatting should preserve final log detail query for test17.sql: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test17.sql"
    );
}

#[test]
fn format_sql_preserves_test19_execution_unit_splitter_final_boss_script() {
    let input = load_test_file("test19.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);

    let expected_lines = vec![
        "CREATE OR REPLACE PACKAGE BODY qt_boss_pkg",
        "g_body_trap CONSTANT VARCHAR2",
        "BODY-END~';",
        "END qt_boss_pkg;",
        "CREATE OR REPLACE PROCEDURE qt_boss_proc",
        "END LOOP outer_loop;",
        "END qt_boss_proc;",
        "CREATE OR REPLACE VIEW qt_boss_view AS",
        "LOG DISTRIBUTION FAIL:",
        "SELECT log_id,",
    ];

    assert_contains_all(&formatted, &expected_lines);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for test19.sql"
    );

    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test19.sql"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool command count for test19.sql"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_boss_pkg")
                && stmt.contains("g_body_trap CONSTANT VARCHAR2")
                && stmt.contains("BODY-END~';")
                && stmt.contains("END qt_boss_pkg")
        }),
        "Formatting should preserve package body execution unit for test19.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_boss_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_boss_proc")
        }),
        "Formatting should preserve standalone procedure execution unit for test19.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("LOG DISTRIBUTION FAIL")
                && stmt.contains("v_lex_cnt")
                && stmt.trim_end().ends_with("END")
        }),
        "Formatting should preserve verification anonymous block for test19.sql: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("SELECT log_id,")
                && stmt.contains("payload_preview")
                && stmt.contains("ORDER BY log_id")
        }),
        "Formatting should preserve final payload preview query for test19.sql: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test19.sql"
    );
}

#[test]
fn format_sql_preserves_mariadb_final_boss_script() {
    let input = load_mariadb_test_file("test1.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test1.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "USE qt_mysql_final_boss",
            "DELIMITER $$",
            "CREATE PROCEDURE sp_run_final_boss()",
            "read_loop: LOOP",
            "END$$",
            "DELIMITER ;",
            "`group`",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mysql/test1.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mysql/test1.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("/*!80000 SET @versioned_comment_executed = 1 */")
        }),
        "Formatting should preserve the MariaDB executable comment statement: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_final_boss")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR 1062")
                && stmt.contains("read_loop: LOOP")
                && stmt.contains("END LOOP;")
                && stmt.contains("END")
        }),
        "Formatting should preserve the main MariaDB procedure execution unit: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("summary_key")
                && stmt.contains("ORDER BY summary_key")
        }),
        "Formatting should preserve the final summary query: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mysql/test1.txt"
    );
}

#[test]
fn format_sql_preserves_mariadb_parser_killer_script() {
    let input = load_mariadb_test_file("test2.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test2.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "USE qt_mysql_parser_killer",
            "DELIMITER $$",
            "DELIMITER //",
            "CREATE PROCEDURE sp_run_parser_killer()",
            "END//",
            "DELIMITER ;",
            "`group`",
            "`rank`",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mysql/test2.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mysql/test2.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("/*!80000 SET @versioned_comment_ok = 1 */")
        }),
        "Formatting should preserve the MariaDB executable comment statement: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_parser_killer")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR 1062")
                && stmt.contains("WITH RECURSIVE node_tree")
                && stmt.contains("ROW_NUMBER")
                && stmt.contains("COMMIT")
        }),
        "Formatting should preserve the main MariaDB procedure execution unit: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("result_key")
                && stmt.contains("ORDER BY result_key")
        }),
        "Formatting should preserve the final result query: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mysql/test2.txt"
    );
}

#[test]
fn format_sql_preserves_mariadb_ultra_final_boss_script() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "USE qt_mysql_ultra_final_boss",
            "DELIMITER $$",
            "DELIMITER //",
            "CREATE PROCEDURE sp_run_ultra_final_boss()",
            "END//",
            "DELIMITER ;",
            "`group`",
            "`rank`",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mysql/test3.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mysql/test3.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("/*!80000 SET @versioned_comment_ok = 1 */")
        }),
        "Formatting should preserve the MariaDB executable comment statement: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_ultra_final_boss")
                && stmt.contains("nested block / CASE mismatch")
                && stmt.contains("WINDOW")
                && stmt.contains("top run id mismatch")
                && stmt.contains("COMMIT")
        }),
        "Formatting should preserve the main MariaDB procedure execution unit: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("summary_key")
                && stmt.contains("ORDER BY `rank`")
                && stmt.contains("summary_key")
        }),
        "Formatting should preserve the final summary query: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mysql/test3.txt"
    );
}

#[test]
fn format_sql_keeps_mariadb_test3_collect_status_counts_select_item_depths() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic_no_cache(&input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Find the SELECT inside sp_collect_status_counts BEGIN block.
    // The procedure body SELECT should have SUM items each on their own line.
    let collect_begin_idx = lines
        .iter()
        .enumerate()
        .find(|(_, line)| {
            line.trim_start() == "BEGIN" && {
                // Must follow sp_collect_status_counts )
                true
            }
        })
        .map(|(i, _)| i)
        .expect("BEGIN in sp_collect_status_counts");

    // Find the SELECT just after the BEGIN
    let select_idx = lines
        .iter()
        .enumerate()
        .skip(collect_begin_idx)
        .find(|(_, line)| line.trim_start() == "SELECT")
        .map(|(i, _)| i)
        .expect("SELECT inside sp_collect_status_counts");

    let sum_done_idx = lines
        .iter()
        .enumerate()
        .skip(select_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("SUM("))
        .map(|(i, _)| i)
        .expect("first SUM( item");

    let select_indent = leading_spaces(lines[select_idx]);
    let sum_indent = leading_spaces(lines[sum_done_idx]);

    assert_eq!(
        sum_indent,
        select_indent + 4,
        "SUM( item should be one level deeper than SELECT (depth select+1), got:\n{}",
        formatted
    );

    // CASE inside SUM should be one deeper than SUM
    let case_idx = lines
        .iter()
        .enumerate()
        .skip(sum_done_idx + 1)
        .find(|(_, line)| line.trim_start() == "CASE")
        .map(|(i, _)| i)
        .expect("CASE inside SUM");
    assert_eq!(
        leading_spaces(lines[case_idx]),
        sum_indent + 4,
        "CASE should be one level deeper than SUM(, got:\n{}",
        formatted
    );

    // WHEN inside CASE should be one deeper than CASE
    let when_idx = lines
        .iter()
        .enumerate()
        .skip(case_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("WHEN status_code = 'DONE'"))
        .map(|(i, _)| i)
        .expect("WHEN inside CASE");
    assert_eq!(
        leading_spaces(lines[when_idx]),
        leading_spaces(lines[case_idx]) + 4,
        "WHEN should be one level deeper than CASE, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_basic_no_cache_is_idempotent_for_mariadb_test2_and_test3() {
    // Cache-bypassing idempotency check for test files that contain SELECT func(CASE ...)
    // patterns. Previously the formatter produced non-idempotent output for these: the
    // first pass kept `SELECT SUM(` on one line and the second pass split it, creating
    // a cycle that only the LRU cache was hiding during single-test runs.
    for name in &["test2.txt", "test3.txt"] {
        let input = load_mariadb_test_file(name);
        let formatted = SqlEditorWidget::format_sql_basic_no_cache(&input);
        let formatted_again = SqlEditorWidget::format_sql_basic_no_cache(&formatted);
        assert_eq!(
            formatted, formatted_again,
            "format_sql_basic must be idempotent for test_mariadb/{name}"
        );
    }
}

#[test]
fn format_sql_cache_and_no_cache_match_for_mariadb_final_boss_v3() {
    let input = load_mariadb_test_file("test6.txt");
    assert!(
        !input.is_empty(),
        "test_mariadb/test6.txt should not be empty"
    );

    let cached = SqlEditorWidget::format_sql_basic_for_db_type(
        &input,
        crate::db::connection::DatabaseType::MySQL,
    );
    let uncached = SqlEditorWidget::format_sql_basic_no_cache_for_db_type(
        &input,
        crate::db::connection::DatabaseType::MySQL,
    );

    assert_eq!(
        cached, uncached,
        "cache and no-cache formatting should match for test_mariadb/test6.txt"
    );
}

#[test]
fn format_sql_preserves_mariadb_final_boss_v2_script() {
    let input = load_mariadb_test_file("test4.txt");
    assert!(
        !input.is_empty(),
        "test_mariadb/test4.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &input,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "USE mdb_finalboss;",
            "DELIMITER $$",
            "CREATE PROCEDURE sp_seed_monster_data()",
            "CREATE PROCEDURE sp_build_monthly_rollup(",
            "END$$",
            "DELIMITER ;",
            "WITH RECURSIVE dept_tree AS (",
            "CALL sp_assert_eq_bigint(",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mariadb/test4.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mariadb/test4.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_seed_monster_data")
                && stmt.contains("WHILE v_day < 35 DO")
                && stmt.contains("SET v_work_date = DATE_ADD('2025-01-01', INTERVAL v_day DAY)")
                && stmt.contains("END WHILE;")
        }),
        "Formatting should preserve the main seed procedure execution unit for test4: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_build_monthly_rollup")
                && stmt.contains("DECLARE EXIT HANDLER FOR SQLEXCEPTION")
                && stmt.contains("ON DUPLICATE KEY UPDATE")
                && stmt.contains("JOIN (")
                && stmt.contains("START TRANSACTION")
                && stmt.contains("COMMIT")
        }),
        "Formatting should preserve the rollup procedure execution unit for test4: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &formatted,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mariadb/test4.txt"
    );
}

#[test]
fn format_sql_preserves_mariadb_ultra_final_boss_v2_script() {
    let input = load_mariadb_test_file("test5.txt");
    assert!(
        !input.is_empty(),
        "test_mariadb/test5.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &input,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "USE mdb_ultra_boss;",
            "DELIMITER $$",
            "DELIMITER //",
            "CREATE PROCEDURE sp_seed_ultra_data()",
            "CREATE PROCEDURE sp_fault_injection_handled()",
            "END//",
            "DELIMITER ;",
            "CALL sp_assert_eq_decimal(",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mariadb/test5.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mariadb/test5.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_seed_ultra_data")
                && stmt.contains("WHILE v_project_id <= 4 DO")
                && stmt.contains("WHILE v_sprint_no <= 3 DO")
                && stmt.contains("END WHILE;")
        }),
        "Formatting should preserve the nested seed procedure loops for test5: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_fault_injection_handled")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR user_error")
                && stmt.contains("GET DIAGNOSTICS CONDITION 1")
                && stmt.contains("INSERT INTO qa_error_capture")
        }),
        "Formatting should preserve diagnostics handler flow for test5: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_rebuild_monthly_stats")
                && stmt.contains("SAVEPOINT sp_one")
                && stmt.contains("ROLLBACK TO SAVEPOINT sp_one")
                && stmt.contains("CALL sp_upsert_one_month_stat")
                && stmt.contains("COMMIT")
        }),
        "Formatting should preserve savepoint recovery flow for test5: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &formatted,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mariadb/test5.txt"
    );
}

#[test]
fn format_sql_preserves_mariadb_final_boss_v3_script() {
    let input = load_mariadb_test_file("test6.txt");
    assert!(
        !input.is_empty(),
        "test_mariadb/test6.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &input,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_contains_all(
        &formatted,
        &[
            "DELIMITER $$",
            "CREATE FUNCTION fn_month_key(p_date DATE)",
            "CREATE FUNCTION fn_order_risk(p_order_id INT)",
            "CREATE PROCEDURE sp_seed_final_boss()",
            "CREATE PROCEDURE sp_validate_final_boss()",
            "CREATE PROCEDURE sp_dynamic_month_pivot(IN p_from DATE, IN p_to DATE)",
            "END$$",
            "DELIMITER ;",
            "WITH RECURSIVE region_tree AS (",
            "JSON_OBJECTAGG(x.segment, x.net_sales) AS segment_to_net_sales",
        ],
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for test_mariadb/test6.txt: {formatted_statements:?}"
    );
    assert_eq!(
        count_script_tool_commands(&formatted_items),
        count_script_tool_commands(&original_items),
        "Formatting changed tool-command count for test_mariadb/test6.txt"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_seed_final_boss")
                && stmt.contains("DECLARE EXIT HANDLER FOR SQLEXCEPTION")
                && stmt.contains("ELSEIF MOD(v_order_id, 4) = 1 THEN")
                && stmt.contains("END IF;")
                && stmt.contains("COMMIT")
        }),
        "Formatting should preserve seed procedure branch/handler flow for test6: {formatted_statements:?}"
    );
    assert!(
        formatted_statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_validate_final_boss")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR NOT FOUND")
                && stmt.contains("SET done = 1;")
                && stmt.contains("OPEN cur;")
                && stmt.contains("END LOOP;")
        }),
        "Formatting should preserve cursor validation flow for test6: {formatted_statements:?}"
    );

    let formatted_again = SqlEditorWidget::format_for_auto_formatting_with_db_type(
        &formatted,
        false,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_mariadb/test6.txt"
    );
}

#[test]
fn format_sql_keeps_mariadb_test1_function_case_and_window_definition_depths() {
    let input = load_mariadb_test_file("test1.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test1.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let lines: Vec<&str> = formatted.lines().collect();

    let case_idx =
        find_line_starting_with(&lines, "CASE UPPER(TRIM(p_currency_code))").expect("CASE line");
    let when_idx = lines
        .iter()
        .enumerate()
        .skip(case_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("WHEN 'USD' THEN"))
        .map(|(idx, _)| idx)
        .expect("WHEN line");
    let value_idx = lines
        .iter()
        .enumerate()
        .skip(when_idx + 1)
        .find(|(_, line)| line.trim_start() == "1.0000")
        .map(|(idx, _)| idx)
        .expect("CASE branch body");

    assert_eq!(
        leading_spaces(lines[when_idx]),
        leading_spaces(lines[case_idx]).saturating_add(4),
        "function CASE WHEN should stay one level deeper than CASE, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[value_idx]),
        leading_spaces(lines[when_idx]).saturating_add(4),
        "function CASE body should stay one level deeper than WHEN, got:\n{}",
        formatted
    );

    let window_idx = find_line_starting_with(&lines, "WINDOW").expect("WINDOW line");
    let named_idx = lines
        .iter()
        .enumerate()
        .skip(window_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("w_emp AS ("))
        .map(|(idx, _)| idx)
        .expect("named WINDOW line");
    let partition_idx = lines
        .iter()
        .enumerate()
        .skip(named_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("PARTITION BY ob.emp_id"))
        .map(|(idx, _)| idx)
        .expect("PARTITION BY line");
    let order_idx = lines
        .iter()
        .enumerate()
        .skip(partition_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("ORDER BY ob.created_at"))
        .map(|(idx, _)| idx)
        .expect("ORDER BY line");
    let close_idx = lines
        .iter()
        .enumerate()
        .skip(order_idx + 1)
        .find(|(_, line)| line.trim() == "),")
        .map(|(idx, _)| idx)
        .expect("WINDOW definition close");

    assert_eq!(
        leading_spaces(lines[named_idx]),
        leading_spaces(lines[window_idx]).saturating_add(4),
        "named WINDOW definition should stay one level deeper than WINDOW, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[partition_idx]),
        leading_spaces(lines[named_idx]).saturating_add(4),
        "WINDOW PARTITION BY should stay one level deeper than the named definition, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[partition_idx]),
        leading_spaces(lines[order_idx]),
        "WINDOW PARTITION BY and ORDER BY should share the same body depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[close_idx]),
        leading_spaces(lines[named_idx]),
        "WINDOW closing line should realign with the named definition, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_test2_loop_and_case_branch_depths() {
    let input = load_mariadb_test_file("test2.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test2.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let lines: Vec<&str> = formatted.lines().collect();

    let while_idx =
        find_line_starting_with(&lines, "WHILE v_i <= 5 DO").expect("WHILE header line");
    let while_body_idx = lines
        .iter()
        .enumerate()
        .skip(while_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("SET v_sum_while ="))
        .map(|(idx, _)| idx)
        .expect("WHILE body line");
    let end_while_idx = lines
        .iter()
        .enumerate()
        .skip(while_body_idx + 1)
        .find(|(_, line)| line.trim_start() == "END WHILE;")
        .map(|(idx, _)| idx)
        .expect("END WHILE line");

    assert_eq!(
        leading_spaces(lines[while_body_idx]),
        leading_spaces(lines[while_idx]).saturating_add(4),
        "WHILE body should stay one level deeper than WHILE, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[end_while_idx]),
        leading_spaces(lines[while_idx]),
        "END WHILE should realign with WHILE, got:\n{}",
        formatted
    );

    let repeat_idx = find_line_starting_with(&lines, "REPEAT").expect("REPEAT line");
    let repeat_body_idx = lines
        .iter()
        .enumerate()
        .skip(repeat_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("SET v_i = v_i + 1;"))
        .map(|(idx, _)| idx)
        .expect("REPEAT body line");
    let end_repeat_idx = lines
        .iter()
        .enumerate()
        .skip(repeat_body_idx + 1)
        .find(|(_, line)| line.trim_start() == "END REPEAT;")
        .map(|(idx, _)| idx)
        .expect("END REPEAT line");

    assert_eq!(
        leading_spaces(lines[repeat_body_idx]),
        leading_spaces(lines[repeat_idx]).saturating_add(4),
        "REPEAT body should stay one level deeper than REPEAT, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[end_repeat_idx]),
        leading_spaces(lines[repeat_idx]),
        "END REPEAT should realign with REPEAT, got:\n{}",
        formatted
    );

    let case_idx = find_line_starting_with(&lines, "CASE v_status_code").expect("cursor CASE line");
    let when_idx = lines
        .iter()
        .enumerate()
        .skip(case_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("WHEN 'DONE' THEN"))
        .map(|(idx, _)| idx)
        .expect("cursor CASE WHEN");
    let body_idx = lines
        .iter()
        .enumerate()
        .skip(when_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("SET v_done_count ="))
        .map(|(idx, _)| idx)
        .expect("cursor CASE body");
    let else_idx = lines
        .iter()
        .enumerate()
        .skip(body_idx + 1)
        .find(|(_, line)| line.trim_start() == "ELSE")
        .map(|(idx, _)| idx)
        .expect("cursor CASE ELSE");

    assert_eq!(
        leading_spaces(lines[when_idx]),
        leading_spaces(lines[case_idx]).saturating_add(4),
        "cursor CASE WHEN should stay one level deeper than CASE, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[body_idx]),
        leading_spaces(lines[when_idx]).saturating_add(4),
        "cursor CASE body should stay one level deeper than WHEN, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[else_idx]),
        leading_spaces(lines[when_idx]),
        "cursor CASE ELSE should align with WHEN, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_test3_nested_case_and_window_sibling_depths() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let lines: Vec<&str> = formatted.lines().collect();

    let nested_block_idx =
        find_line_starting_with(&lines, "nested_block:").expect("nested block label");
    let nested_case_idx = lines
        .iter()
        .enumerate()
        .skip(nested_block_idx + 1)
        .find(|(_, line)| line.trim_start() == "CASE")
        .map(|(idx, _)| idx)
        .expect("nested CASE line");
    let nested_when_idx = lines
        .iter()
        .enumerate()
        .skip(nested_case_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("WHEN v_local = 1 THEN"))
        .map(|(idx, _)| idx)
        .expect("nested CASE WHEN");
    let nested_body_idx = lines
        .iter()
        .enumerate()
        .skip(nested_when_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("SET v_local = v_local + 9;"))
        .map(|(idx, _)| idx)
        .expect("nested CASE body");

    assert_eq!(
        leading_spaces(lines[nested_when_idx]),
        leading_spaces(lines[nested_case_idx]).saturating_add(4),
        "nested CASE WHEN should stay one level deeper than CASE, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[nested_body_idx]),
        leading_spaces(lines[nested_when_idx]).saturating_add(4),
        "nested CASE body should stay one level deeper than WHEN, got:\n{}",
        formatted
    );

    let window_idx = find_line_starting_with(&lines, "WINDOW").expect("WINDOW line");
    let named_idx = lines
        .iter()
        .enumerate()
        .skip(window_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("w_owner AS ("))
        .map(|(idx, _)| idx)
        .expect("named WINDOW line");
    let partition_idx = lines
        .iter()
        .enumerate()
        .skip(named_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("PARTITION BY s.owner_name"))
        .map(|(idx, _)| idx)
        .expect("PARTITION BY line");
    let sibling_idx = lines
        .iter()
        .enumerate()
        .skip(partition_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("w_owner_running AS ("))
        .map(|(idx, _)| idx)
        .expect("WINDOW sibling line");

    assert_eq!(
        leading_spaces(lines[named_idx]),
        leading_spaces(lines[window_idx]).saturating_add(4),
        "named WINDOW definition should stay one level deeper than WINDOW, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[partition_idx]),
        leading_spaces(lines[named_idx]).saturating_add(4),
        "WINDOW body should stay one level deeper than the named definition, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[sibling_idx]),
        leading_spaces(lines[named_idx]),
        "WINDOW siblings should realign to the named definition depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_helper_procedure_closing_paren_aligned() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let expected = r#"CREATE PROCEDURE sp_collect_status_counts(
    OUT p_done_cnt INT,
    OUT p_running_cnt INT,
    OUT p_other_cnt INT
)
BEGIN"#;

    assert!(
        formatted.contains(expected),
        "helper procedure parameter closing paren should stay aligned with the procedure header, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_trigger_end_delimiter_aligned() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());
    let trigger_idx = lines
        .iter()
        .position(|line| line.trim_start() == "CREATE TRIGGER trg_run_case_ai")
        .expect("formatted output should contain trg_run_case_ai");
    let begin_idx = lines
        .iter()
        .enumerate()
        .skip(trigger_idx + 1)
        .find(|(_, line)| line.trim_start() == "BEGIN")
        .map(|(idx, _)| idx)
        .expect("formatted output should contain trigger BEGIN");
    let end_idx = lines
        .iter()
        .enumerate()
        .skip(begin_idx + 1)
        .find(|(_, line)| line.trim_start() == "END$$")
        .map(|(idx, _)| idx)
        .expect("formatted output should contain trigger END$$");

    assert_eq!(
        indent(lines[end_idx]),
        indent(lines[begin_idx]),
        "trigger END$$ should align with BEGIN, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_trigger_cast_type_inline() {
    let input = load_mariadb_test_file("test3.txt");
    assert!(
        !input.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);

    assert!(
        formatted.contains(
            "COALESCE(CAST(JSON_LENGTH(JSON_EXTRACT(NEW.payload, '$.steps')) AS CHAR), 'null')"
        ),
        "trigger CAST type should stay inline after AS, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_anonymous_block_with_local_record_bind_helpers_as_single_execution_unit() {
    let input = r#"DECLARE
    v_cursor_id INTEGER;
    v_sql CLOB;
    v_rows INTEGER;
    v_desc_tab DBMS_SQL.DESC_TAB;
    v_col_count INTEGER;
    v_varchar VARCHAR2 (4000);
    v_number NUMBER;
    v_date DATE;
    TYPE bind_rec_t IS RECORD (NAME VARCHAR2 (30), dtype VARCHAR2 (30), value VARCHAR2 (4000));
    TYPE bind_tab_t IS TABLE OF bind_rec_t;
    l_binds bind_tab_t := bind_tab_t ();
    PROCEDURE add_bind (p_name VARCHAR2, p_type VARCHAR2, p_val VARCHAR2) IS
    BEGIN
        l_binds.EXTEND;
        l_binds (l_binds.COUNT) := bind_rec_t (p_name, p_type, p_val);
    END;

BEGIN
    add_bind (':dept_id', 'NUMBER', '50');
    add_bind (':status', 'VARCHAR2', 'ACTIVE');
    add_bind (':min_sal', 'NUMBER', '5000');
    v_sql := 'SELECT employee_id, first_name || '' '' || last_name AS name, ' || 'salary, hire_date ' || 'FROM employees ' || 'WHERE department_id = :dept_id ' || 'AND status = :status ' || 'AND salary >= :min_sal ' || 'ORDER BY salary DESC';
    v_cursor_id := DBMS_SQL.OPEN_CURSOR;
    DBMS_SQL.PARSE (v_cursor_id, v_sql, DBMS_SQL.NATIVE);
    FOR i IN 1..l_binds.COUNT LOOP
        CASE l_binds (i).dtype
            WHEN 'NUMBER' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, TO_NUMBER (l_binds (i).value));
            WHEN 'VARCHAR2' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, l_binds (i).value);
            WHEN 'DATE' THEN
                DBMS_SQL.BIND_VARIABLE (v_cursor_id, l_binds (i).NAME, TO_DATE (l_binds (i).value, 'YYYY-MM-DD'));
        END CASE;
    END LOOP;
    DBMS_SQL.DESCRIBE_COLUMNS (v_cursor_id, v_col_count, v_desc_tab);
    FOR i IN 1..v_col_count LOOP
        CASE v_desc_tab (i).col_type
            WHEN 1 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_varchar, 4000);
            WHEN 2 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_number);
            WHEN 12 THEN
                DBMS_SQL.DEFINE_COLUMN (v_cursor_id, i, v_date);
        END CASE;
    END LOOP;
    v_rows := DBMS_SQL.EXECUTE (v_cursor_id);
    WHILE DBMS_SQL.FETCH_ROWS (v_cursor_id) > 0 LOOP
        FOR i IN 1..v_col_count LOOP
            CASE v_desc_tab (i).col_type
                WHEN 1 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_varchar);
                    DBMS_OUTPUT.PUT (RPAD (NVL (v_varchar, 'NULL'), 30));
                WHEN 2 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_number);
                    DBMS_OUTPUT.PUT (RPAD (TO_CHAR (v_number), 15));
                WHEN 12 THEN
                    DBMS_SQL.COLUMN_VALUE (v_cursor_id, i, v_date);
                    DBMS_OUTPUT.PUT (RPAD (TO_CHAR (v_date, 'YYYY-MM-DD'), 15));
            END CASE;
        END LOOP;
        DBMS_OUTPUT.NEW_LINE;
    END LOOP;
    DBMS_SQL.CLOSE_CURSOR (v_cursor_id);
EXCEPTION
    WHEN OTHERS THEN
        IF DBMS_SQL.IS_OPEN (v_cursor_id) THEN
            DBMS_SQL.CLOSE_CURSOR (v_cursor_id);
        END IF;
        RAISE;
END;"#;

    let original_items = QueryExecutor::split_script_items(input);
    let format_items = QueryExecutor::split_format_items(input);
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        count_script_statements(&original_items),
        1,
        "input should already be one execution unit"
    );
    assert_eq!(
        format_items
            .iter()
            .filter(|item| matches!(item, FormatItem::Statement(_)))
            .count(),
        1,
        "format splitter must keep the anonymous block intact before rendering"
    );
    assert_eq!(
        count_script_statements(&formatted_items),
        1,
        "formatting must preserve the anonymous block as one execution unit, got: {formatted_statements:?}\nformatted:\n{formatted}"
    );
    assert!(
        !formatted.contains("END;\n\nBEGIN"),
        "formatter must not insert a statement separator between local procedure END and outer BEGIN, got:\n{formatted}"
    );
    assert!(
        formatted_statements
            .first()
            .is_some_and(|stmt| stmt.starts_with("DECLARE") && stmt.contains("PROCEDURE add_bind") && stmt.contains("DBMS_SQL.CLOSE_CURSOR")),
        "formatted statement lost anonymous block structure: {formatted_statements:?}\nformatted:\n{formatted}"
    );
}

#[test]
fn format_sql_preserves_oracle_format_final_boss_v2_and_depth_indentation() {
    let input = load_test_file("oracle_format_final_boss_v2.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);

    let expected_lines = vec![
        "-- FORMATTER FINAL BOSS : Oracle SQL / PL-SQL One-Script Stress Set",
        "CREATE OR REPLACE PACKAGE BODY qt_fmt_pkg",
        "OPEN l_rc FOR",
        "WITH base",
        "OUTER APPLY",
        "MODEL",
        "q'[TOP_IN_DEPT]'",
        "WITHIN GROUP (ORDER BY b2.bonus_year, b2.bonus_type)",
        "ORDER BY a.audit_id;",
    ];
    assert_contains_all(&formatted, &expected_lines);

    let input_slashes = count_slash_lines(&input);
    let output_slashes = count_slash_lines(&formatted);
    assert_eq!(
        input_slashes, output_slashes,
        "Slash terminator count differs for oracle_format_final_boss_v2.sql"
    );

    let original_items = QueryExecutor::split_script_items(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    assert_eq!(
        count_script_statements(&formatted_items),
        count_script_statements(&original_items),
        "Formatting changed execution statement count for oracle_format_final_boss_v2.sql"
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let find_line = |needle: &str| {
        lines
            .iter()
            .position(|line| line.trim_start().starts_with(needle))
    };

    let open_idx = find_line("OPEN l_rc FOR").unwrap_or(0);
    let with_idx = find_line("WITH base").unwrap_or(0);
    assert!(
        indent(lines[with_idx]) > indent(lines[open_idx]),
        "WITH inside OPEN FOR should be indented deeper than OPEN l_rc FOR, got:\n{formatted}"
    );

    let case_idx = find_line("CASE").unwrap_or(0);
    let when_idx = find_line("WHEN e.salary >= 100000 THEN").unwrap_or(0);
    assert!(
        indent(lines[when_idx]) > indent(lines[case_idx]),
        "WHEN branch should be indented under CASE, got:\n{formatted}"
    );

    let update_set_idx = find_line("SET t.action_name  = 'UPSERT',").unwrap_or(0);
    let update_idx = find_line("UPDATE").unwrap_or(0);
    assert!(
        indent(lines[update_set_idx]) > indent(lines[update_idx]),
        "UPDATE SET assignments should stay deeper than UPDATE keyword, got:\n{formatted}"
    );

    let apply_idx = find_line("OUTER APPLY").unwrap_or(0);
    let select_idx = find_line("SELECT b.bonus_type AS top_bonus_type,").unwrap_or(0);
    assert!(
        indent(lines[select_idx]) > indent(lines[apply_idx]),
        "OUTER APPLY inline SELECT should be indented deeper than OUTER APPLY, got:\n{formatted}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for oracle_format_final_boss_v2.sql"
    );
}

#[test]
fn format_sql_preserves_test_045_execution_unit_inside_oracle_splitter_final_boss_script() {
    let input = load_test_file("oracle splitter final boss test.sql");
    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let formatted_statements: Vec<&str> = formatted_items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    let matched = formatted_statements.iter().find(|stmt| {
        stmt.starts_with("DECLARE")
            && stmt.contains("TYPE bind_rec_t IS RECORD")
            && stmt.contains("PROCEDURE add_bind")
            && stmt.contains("DBMS_SQL.DESCRIBE_COLUMNS")
    });

    assert!(
        matched.is_some(),
        "formatted splitter should keep TEST-045 DBMS_SQL block as one statement, got: {formatted_statements:?}"
    );
}

#[test]
fn format_sql_keeps_with_attached_to_single_letter_cte_name() {
    let input = "WITH\n    r\n    AS\n    (\n        SELECT 1 AS id\n        FROM dual\n    )\nSELECT *\nFROM r\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("WITH r AS (") || formatted.contains("WITH\n    r\n    AS ("),
        "single-letter CTE name must remain attached to WITH, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("WITH;\n\nr"),
        "single-letter CTE name must not be split into WITH + standalone r, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_indents_nested_sql_case_under_then_and_order_by_clause() {
    let input = "DECLARE\n    l_rc SYS_REFCURSOR;\nBEGIN\n    OPEN l_rc FOR\n        SELECT\n            CASE\n                WHEN salary >= 100000 THEN\n                    CASE\n                        WHEN status_cd = 'ACTIVE' THEN 'HIGH_ACTIVE'\n                        ELSE 'HIGH_NOT_ACTIVE'\n                    END\n                ELSE 'OTHER'\n            END AS band_label,\n            emp_id\n        FROM emp\n        ORDER BY\n            CASE\n                WHEN salary >= 100000 THEN 1\n                WHEN salary >= 80000 THEN 2\n                ELSE 3\n            END,\n            emp_id;\nEND;\n/";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let outer_when_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHEN salary >= 100000 THEN"))
        .unwrap_or(0);
    let nested_case_idx = lines
        .iter()
        .enumerate()
        .skip(outer_when_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "CASE")
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let nested_else_idx = lines
        .iter()
        .position(|line| line.trim_start() == "ELSE 'HIGH_NOT_ACTIVE'")
        .unwrap_or(0);
    let order_by_idx = lines
        .iter()
        .position(|line| line.trim_start() == "ORDER BY")
        .unwrap_or(0);
    let order_case_idx = lines
        .iter()
        .enumerate()
        .skip(order_by_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "CASE")
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    assert!(
        indent(lines[nested_case_idx]) > indent(lines[outer_when_idx]),
        "nested SQL CASE after THEN should indent deeper than parent WHEN branch, got:\n{}",
        formatted
    );
    assert!(
        indent(lines[nested_else_idx]) > indent(lines[outer_when_idx]),
        "nested SQL CASE ELSE should stay aligned inside nested CASE, got:\n{}",
        formatted
    );
    assert!(
        indent(lines[order_case_idx]) > indent(lines[order_by_idx]),
        "ORDER BY CASE should indent deeper than ORDER BY header, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_breaks_outer_apply_after_from_source() {
    let input = "SELECT e.emp_id, x.top_bonus_type FROM qt_fmt_emp e OUTER APPLY (SELECT b.bonus_type AS top_bonus_type FROM qt_fmt_bonus b WHERE b.emp_id = e.emp_id FETCH FIRST 1 ROW ONLY) x ORDER BY e.emp_id;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("FROM qt_fmt_emp e\nOUTER APPLY (")
            || formatted.contains("FROM qt_fmt_emp e\n    OUTER APPLY ("),
        "OUTER APPLY should start on its own clause line after FROM source, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_whenever_sqlerror_options() {
    let input = [
        "WHENEVER SQLERROR EXIT SQL.SQLCODE",
        "WHENEVER SQLERROR EXIT FAILURE ROLLBACK",
        "WHENEVER SQLERROR EXIT SUCCESS",
        "WHENEVER SQLERROR EXIT WARNING",
        "WHENEVER SQLERROR EXIT 1",
        "WHENEVER SQLERROR CONTINUE",
        "WHENEVER SQLERROR CONTINUE ROLLBACK",
    ]
    .join("\n");

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let expected_lines = vec![
        "WHENEVER SQLERROR EXIT SQL.SQLCODE",
        "WHENEVER SQLERROR EXIT FAILURE ROLLBACK",
        "WHENEVER SQLERROR EXIT SUCCESS",
        "WHENEVER SQLERROR EXIT WARNING",
        "WHENEVER SQLERROR EXIT 1",
        "WHENEVER SQLERROR CONTINUE",
        "WHENEVER SQLERROR CONTINUE ROLLBACK",
    ];

    assert_contains_all(&formatted, &expected_lines);

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for WHENEVER SQLERROR variants"
    );
}

#[test]
fn format_sql_keeps_if_alias_member_access_intact() {
    let input = "select if.a, if.b from tablename if";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("IF.a") && formatted.contains("IF.b"),
        "IF alias member access should be preserved, got:\n{}",
        formatted
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should remain idempotent for IF alias member access"
    );
}
#[test]
fn format_sql_keeps_update_alias_named_if_inline() {
    let input = "update sales if set if.amount = if.amount + 1 where if.id = 1";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("UPDATE sales IF")
            && formatted.contains("IF.amount")
            && formatted.contains("WHERE IF.id = 1;"),
        "UPDATE alias IF should remain inline and usable in member access, got:
{}",
        formatted
    );
    assert!(
        !formatted.contains("\nIF\n") && !formatted.contains("\n    IF\n"),
        "UPDATE alias IF should not be treated as block keyword, got:
{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_merge_into_alias_named_if_inline() {
    let input = "merge into sales if using dual d on (if.id = d.dummy) when matched then update set if.amount = 0";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("MERGE INTO sales IF")
            && formatted.contains("IF.id = d.dummy")
            && formatted.contains("IF.amount = 0;"),
        "MERGE INTO alias IF should remain inline, got:
{}",
        formatted
    );
    assert!(
        !formatted.contains("\nIF\n") && !formatted.contains("\n    IF\n"),
        "MERGE INTO alias IF should not be treated as block keyword, got:
{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_merge_using_alias_named_if_inline() {
    let input = "merge into sales t using source_table if on (t.id = if.id) when matched then update set t.amount = if.amount";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("USING source_table IF")
            && formatted.contains("IF.id")
            && formatted.contains("IF.amount"),
        "MERGE USING alias IF should remain inline, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("\nIF\n") && !formatted.contains("\n    IF\n"),
        "MERGE USING alias IF should not be treated as block keyword, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_delete_alias_named_if_inline() {
    let input = "delete from sales if where if.id = 1";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("DELETE FROM sales IF") && formatted.contains("WHERE IF.id = 1;"),
        "DELETE alias IF should remain inline and usable in member access, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("DELETE\nFROM sales IF")
            && !formatted.contains("\nIF\n")
            && !formatted.contains("\n    IF\n"),
        "DELETE alias IF should not be treated as block keyword, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_breaks_minified_package_body_members() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg AS PROCEDURE p IS BEGIN NULL; END; FUNCTION f RETURN NUMBER IS BEGIN RETURN 1; END; END pkg;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("PACKAGE BODY pkg AS\n    PROCEDURE p IS"),
        "Package body should break before first procedure, got: {}",
        formatted
    );
    assert!(
        formatted.contains("END;\n\n    FUNCTION f RETURN NUMBER IS"),
        "Package body members should be separated by blank line, got: {}",
        formatted
    );
}

#[test]
fn format_sql_breaks_semicolon_separated_declare_statements() {
    let input =
        "DECLARE v_old_sal NUMBER; v_new_sal NUMBER; BEGIN v_old_sal := 1; v_new_sal := 2; END;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("DECLARE\n    v_old_sal NUMBER;\n    v_new_sal NUMBER;\nBEGIN"),
        "semicolon-separated DECLARE statements should each break onto their own line, got: {formatted}"
    );
    assert!(
        formatted.contains("BEGIN\n    v_old_sal := 1;\n    v_new_sal := 2;\nEND;"),
        "semicolon-separated BEGIN body statements should each break onto their own line, got: {formatted}"
    );
}

#[test]
fn format_sql_breaks_semicolon_separated_package_body_declarations() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg AS v_one NUMBER; v_two NUMBER; PROCEDURE p IS BEGIN NULL; END p; END pkg;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "PACKAGE BODY pkg AS\n    v_one NUMBER;\n    v_two NUMBER;\n    PROCEDURE p IS"
        ),
        "semicolon-separated package body declarations should each break onto their own line, got: {formatted}"
    );
    assert!(
        formatted.contains("PROCEDURE p IS\n    BEGIN\n        NULL;\n    END p;"),
        "package body routine body should remain structured after semicolon splitting, got: {formatted}"
    );
}

#[test]
fn format_sql_breaks_next_statement_even_with_inline_block_comment_after_semicolon() {
    let input = "BEGIN v_val := 0; /* keep */ v_val := 1; END;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("BEGIN\n    v_val := 0;\n    /* keep */\n    v_val := 1;\nEND;"),
        "statement separators should still break before the next code line even when an inline block comment follows the semicolon, got: {formatted}"
    );
}

#[test]
fn format_sql_keeps_nested_begin_depth_inside_package_body_procedure() {
    let input = "create package body a as\nprocedure b (c in number) is\nd number := 0;\nbegin\nbegin\nv := v\nend\nend b;\nend a;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("        END\n    END b;"),
        "outer END should keep procedure depth, got: {formatted}"
    );
}

#[test]
fn format_sql_keeps_if_and_begin_aligned_in_nested_package_body_blocks() {
    let input = "create package body pkg as procedure procname is begin begin null; end; if 1 = 1 then null; end if; end procname; end pkg;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "    BEGIN\n        BEGIN\n            NULL;\n        END;\n        IF 1 = 1 THEN\n            NULL;\n        END IF;\n    END procname;"
        ),
        "nested BEGIN/IF indentation should stay at the same procedure depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_loop_and_named_end_aligned_in_nested_package_body_blocks() {
    let input = "create package body pkg as procedure procname is begin begin null; end; loop null; end loop; end procname; end pkg;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "    BEGIN\n        BEGIN\n            NULL;\n        END;\n        LOOP\n            NULL;\n        END LOOP;\n    END procname;"
        ),
        "nested BEGIN/LOOP indentation should stay at the same procedure depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_preserves_oracle_labels() {
    // Test <<loop_label>> preservation
    let input = "<<outer_loop>>\nFOR i IN 1..10 LOOP\n<<inner_loop>>\nFOR j IN 1..5 LOOP\nNULL;\nEND LOOP inner_loop;\nEND LOOP outer_loop;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    // Labels should be preserved without extra spaces
    assert!(
        formatted.contains("<<outer_loop>>"),
        "Label <<outer_loop>> should be preserved, got: {}",
        formatted
    );
    assert!(
        formatted.contains("<<inner_loop>>"),
        "Label <<inner_loop>> should be preserved, got: {}",
        formatted
    );

    // Idempotent test
    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for labels"
    );
}

#[test]
fn format_sql_preserves_q_quoted_strings() {
    // Test q'[...]' quote literal preservation
    let cases = [
        ("SELECT q'[It's a test]' FROM dual", "q'[It's a test]'"),
        ("SELECT q'{Hello World}' FROM dual", "q'{Hello World}'"),
        (
            "SELECT q'(Text with 'quotes')' FROM dual",
            "q'(Text with 'quotes')'",
        ),
        (
            "SELECT q'<Value with <brackets>>'",
            "q'<Value with <brackets>>'",
        ),
        (
            "SELECT Q'!Delimiter test!' FROM dual",
            "Q'!Delimiter test!'",
        ),
    ];

    for (input, expected_literal) in cases {
        let formatted = SqlEditorWidget::format_sql_basic(input);
        assert!(
            formatted.contains(expected_literal),
            "Q-quoted literal {} should be preserved in: {}",
            expected_literal,
            formatted
        );

        // Idempotent test
        let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
        assert_eq!(
            formatted, formatted_again,
            "Formatting should be idempotent for q-quoted string: {}",
            input
        );
    }
}

#[test]
fn format_sql_preserves_combined_special_syntax() {
    // Test combination of labels and q-quoted strings
    let input = r#"<<process_data>>
BEGIN
v_sql := q'[SELECT * FROM table WHERE name = 'test']';
EXECUTE IMMEDIATE v_sql;
END;
"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("<<process_data>>"),
        "Label should be preserved"
    );
    assert!(
        formatted.contains("q'[SELECT * FROM table WHERE name = 'test']'"),
        "Q-quoted string should be preserved exactly"
    );
}

#[test]
fn format_sql_preserves_nq_quoted_strings() {
    // Test nq'[...]' (National Character q-quoted strings)
    let test_cases = [
        (
            "SELECT nq'[한글 문자열]' FROM dual",
            "nq'[한글 문자열]'",
            "basic nq'[...]' preservation",
        ),
        (
            "SELECT NQ'[UPPERCASE]' FROM dual",
            "NQ'[UPPERCASE]'",
            "uppercase NQ'[...]' preservation",
        ),
        (
            "SELECT Nq'[mixed case]' FROM dual",
            "Nq'[mixed case]'",
            "mixed case Nq'[...]' preservation",
        ),
        (
            "SELECT nq'(parentheses)' FROM dual",
            "nq'(parentheses)'",
            "nq'(...)' with parentheses",
        ),
        (
            "SELECT nq'{braces}' FROM dual",
            "nq'{braces}'",
            "nq'{...}' with braces",
        ),
        (
            "SELECT nq'<angle brackets>' FROM dual",
            "nq'<angle brackets>'",
            "nq'<...>' with angle brackets",
        ),
        (
            "SELECT nq'!custom!' FROM dual",
            "nq'!custom!'",
            "nq'!...!' with custom delimiter",
        ),
    ];

    for (input, expected, description) in test_cases {
        let formatted = SqlEditorWidget::format_sql_basic(input);
        assert!(
            formatted.contains(expected),
            "{}: expected '{}' in formatted output, got: {}",
            description,
            expected,
            formatted
        );
    }
}

#[test]
fn format_sql_preserves_nq_quote_with_semicolon() {
    // Test that semicolons inside nq'...' are preserved
    let input = "SELECT nq'[text with ; semicolon]' FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains("nq'[text with ; semicolon]'"),
        "nq'...' with semicolon should be preserved exactly, got: {}",
        formatted
    );
}

#[test]
fn format_sql_preserves_mixed_q_and_nq_quotes() {
    // Test both q'...' and nq'...' in same statement
    let input = "SELECT q'[regular]', nq'[national]' FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains("q'[regular]'"),
        "q'...' should be preserved, got: {}",
        formatted
    );
    assert!(
        formatted.contains("nq'[national]'"),
        "nq'...' should be preserved, got: {}",
        formatted
    );
}

#[test]
fn tokenize_sql_handles_nq_quotes() {
    // Direct test of tokenization for nq'...'
    let sql = "SELECT nq'[test string]' FROM dual";
    let tokens = SqlEditorWidget::tokenize_sql(sql);

    // Should have tokens: SELECT, nq'[test string]', FROM, dual
    let has_nq_string = tokens.iter().any(|t| {
        if let SqlToken::String(s) = t {
            s.contains("nq'[test string]'")
        } else {
            false
        }
    });
    assert!(
        has_nq_string,
        "Tokenizer should produce String token for nq'[...]', got: {:?}",
        tokens
    );
}

#[test]
fn format_sql_places_newline_after_inline_block_comment() {
    let input = "/* 헤더 주석 */SELECT 1 FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("/* 헤더 주석 */\nSELECT 1\nFROM DUAL;"),
        "Inline block comment should be followed by newline before SQL, got: {}",
        formatted
    );
}

#[test]
fn format_sql_does_not_merge_end_statement_with_following_if() {
    let input = "BEGIN\nNULL;\nEND;\nIF 1 = 1 THEN\nNULL;\nEND IF;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("END;\n\nIF 1 = 1 THEN"),
        "END; and following IF must remain separate, got: {}",
        formatted
    );
}

#[test]
fn format_sql_preserves_newline_after_block_comment_end() {
    let input = "SELECT 1 /* trailing */\nFROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("/* trailing */\nFROM DUAL;"),
        "newline after */ should be preserved, got: {}",
        formatted
    );
}

#[test]
fn format_sql_preserves_newline_before_line_comment() {
    let input = "SELECT 1\n-- comment\nFROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT 1\n-- comment\nFROM DUAL;"),
        "newline before -- should be preserved, got: {}",
        formatted
    );
}

#[test]
fn format_sql_preserves_newline_before_block_comment() {
    let input = "SELECT 1\n/* comment */\nFROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT 1\n/* comment */\nFROM DUAL;"),
        "newline before /* should be preserved, got: {}",
        formatted
    );
}

#[test]
fn format_sql_multiline_block_comment_is_separated_from_previous_query() {
    let input = "SELECT 1 FROM dual; /* multi\nline\ncomment */ SELECT 2 FROM dual;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("FROM DUAL;\n\n/* multi\nline\ncomment */\nSELECT 2\nFROM DUAL;"),
        "multiline block comment should not stick to surrounding queries, got: {}",
        formatted
    );
}

#[test]
fn format_sql_keeps_sql_after_multiline_block_comment_closing_line() {
    let input = "/* head\ncomment */ SELECT 1 FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("/* head\ncomment */\nSELECT 1\nFROM DUAL;"),
        "SQL after multiline block comment should be formatted as SQL, got: {}",
        formatted
    );
}

#[test]
fn format_sql_splits_sql_after_single_line_block_comment_closing_token() {
    let input = "/* banner */ SELECT col1, col2 FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "/* banner */
SELECT col1,
    col2
FROM DUAL;"
        ),
        "SQL after single-line block comment should be parsed as SQL statement, got: {}",
        formatted
    );
}

#[test]
fn format_sql_splits_sql_after_multiline_block_comment_closing_token() {
    let input = "/* banner
comment */ SELECT col1, col2 FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "/* banner
comment */
SELECT col1,
    col2
FROM DUAL;"
        ),
        "SQL after multiline block comment should be parsed as SQL statement, got: {}",
        formatted
    );
}

#[test]
fn format_sql_recognizes_prompt_after_leading_block_comment_on_same_line() {
    let input = "/* banner */ PROMPT hello";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "/* banner */

PROMPT hello"
        ),
        "PROMPT after leading block comment should remain SQL*Plus command, got: {}",
        formatted
    );
}

#[test]
fn format_sql_recognizes_slash_after_leading_block_comment_on_same_line() {
    let input = "/* banner */ /";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "/* banner */
/"
        ),
        "Slash command after leading block comment should remain separate item, got: {}",
        formatted
    );
}

#[test]
fn format_sql_recognizes_tool_command_after_multiline_block_comment_closing_line() {
    let input = "/* banner
comment */ CLEAR BREAKS";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "/* banner
comment */

CLEAR BREAKS"
        ),
        "Tool command after multiline block comment should remain SQL*Plus command, got: {}",
        formatted
    );
}

#[test]
fn format_sql_indents_select_list_item_starting_with_parenthesis() {
    let input = "SELECT (a + b) AS sum_value, c FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT\n    (a + b) AS sum_value,"),
        "Select list item starting with '(' should be indented under SELECT, got: {}",
        formatted
    );
}

#[test]
fn format_sql_indents_case_expression_inside_select_clause() {
    let input = "SELECT CASE WHEN a = 1 THEN 'Y' ELSE 'N' END AS flag FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT\n    CASE\n        WHEN a = 1 THEN 'Y'"),
        "CASE inside SELECT should start deeper than SELECT and WHEN should be deeper than CASE, got: {}",
        formatted
    );
}

#[test]
fn format_sql_case_when_does_not_insert_extra_blank_lines() {
    let input =
        "SELECT CASE WHEN a = 1 THEN 'A' WHEN a = 2 THEN 'B' ELSE 'C' END AS flag FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    let expected = [
        "SELECT",
        "    CASE",
        "        WHEN a = 1 THEN 'A'",
        "        WHEN a = 2 THEN 'B'",
        "        ELSE 'C'",
        "    END AS flag",
        "FROM DUAL;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_open_cursor_for_select_indentation() {
    let input = r#"BEGIN
OPEN p_rc
FOR
SELECT empno,
ename,
deptno,
salary
FROM oqt_emp
WHERE deptno = p_deptno
ORDER BY empno;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    OPEN p_rc FOR",
        "        SELECT empno,",
        "            ename,",
        "            deptno,",
        "            salary",
        "        FROM oqt_emp",
        "        WHERE deptno = p_deptno",
        "        ORDER BY empno;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_open_cursor_for_with_clause() {
    let input = r#"BEGIN
OPEN p_rc
FOR
WITH cte AS (
    SELECT empno,
        deptno
    FROM oqt_t_emp
)
SELECT empno,
    deptno
FROM cte;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines = formatted.lines().collect::<Vec<_>>();

    let open_idx = lines
        .iter()
        .position(|line| line.contains("OPEN P_RC FOR"))
        .or_else(|| lines.iter().position(|line| line.contains("OPEN p_rc FOR")))
        .expect("expected OPEN ... FOR line");

    let with_idx = lines
        .iter()
        .position(|line| {
            let upper = line.trim_start().to_ascii_uppercase();
            upper.starts_with("WITH CTE AS (") || upper.contains("OPEN P_RC FOR WITH CTE AS (")
        })
        .expect("expected WITH CTE line");

    let main_from_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_ascii_uppercase()
                .starts_with("FROM CTE")
        })
        .expect("expected main SELECT FROM line");

    let cte_from_idx = lines
        .iter()
        .position(|line| line.to_ascii_uppercase().contains("FROM OQT_T_EMP"))
        .expect("expected CTE body FROM line");

    let open_indent = lines[open_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    let with_indent = lines[with_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    let with_line = lines[with_idx].to_ascii_uppercase();
    let main_from_indent = lines[main_from_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();

    if with_line.contains("OPEN P_RC FOR") {
        assert!(
            with_indent > open_indent,
            "OPEN ... FOR WITH should still indent WITH"
        );
    } else {
        assert_eq!(with_idx, open_idx + 1, "WITH should follow OPEN FOR");
    }

    assert!(with_line.trim_start().contains("WITH CTE AS ("));
    assert!(
        main_from_indent >= with_indent,
        "FROM CTE should align with/inside main SELECT depth"
    );
    assert_eq!(
        lines[cte_from_idx].trim_start().to_ascii_uppercase(),
        "FROM OQT_T_EMP"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("OPEN p_rc FOR") || line.contains("OPEN P_RC FOR")),
        "OPEN ... FOR should remain"
    );
}

#[test]
fn format_sql_fetch_into_list_indentation() {
    let input = r#"BEGIN
FETCH c
INTO v_empno,
v_ename,
v_dept,
v_sal;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    FETCH c",
        "    INTO v_empno,",
        "        v_ename,",
        "        v_dept,",
        "        v_sal;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_insert_into_together() {
    let input = "INSERT\nINTO oqt_call_log (id, tag, msg, n1)\nVALUES (1, 'T', 'M', 10)";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "INSERT INTO oqt_call_log (id, tag, msg, n1)",
        "VALUES (1, 'T', 'M', 10);",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_select_and_into_exact_keyword_lines_keep_dml_indentation() {
    let input = r#"BEGIN
SELECT
CASE
WHEN 1 = 1 THEN 'Y'
ELSE 'N'
END
INTO
v_flag
FROM dual;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT",
        "        CASE",
        "            WHEN 1 = 1 THEN 'Y'",
        "            ELSE 'N'",
        "        END",
        "    INTO v_flag",
        "    FROM DUAL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_select_into_does_not_leak_extra_depth_to_next_statement() {
    let input = r#"BEGIN
SELECT col
INTO
v_col;
IF 1 = 1 THEN
NULL;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT col",
        "    INTO v_col;",
        "    IF 1 = 1 THEN",
        "        NULL;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_select_into_with_connect_by_keeps_clause_depth() {
    let input = r#"BEGIN
SELECT
CASE
WHEN LEVEL = 1 THEN 'ROOT'
ELSE 'CHILD'
END
INTO
v_kind
FROM dual
START WITH 1 = 1
CONNECT BY PRIOR 1 = 1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT",
        "        CASE",
        "            WHEN LEVEL = 1 THEN 'ROOT'",
        "            ELSE 'CHILD'",
        "        END",
        "    INTO v_kind",
        "    FROM DUAL",
        "    START WITH 1 = 1",
        "    CONNECT BY PRIOR 1 = 1;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_select_into_with_union_stops_into_extra_indent() {
    let input = r#"BEGIN
SELECT col1
INTO
v_col
FROM t1
UNION ALL
SELECT col2
FROM t2;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT col1",
        "    INTO v_col",
        "    FROM t1",
        "    UNION ALL",
        "    SELECT col2",
        "    FROM t2;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_where_exists_and_not_exists_layout_regression() {
    let input = "SELECT * FROM asdf WHERE EXISTS (SELECT 1 FROM oqt_t_order_item oi WHERE oi.order_id = v.order_id AND oi.sku LIKE 'SKU-%') AND NOT EXISTS (SELECT 1 FROM oqt_t_order_item oi WHERE oi.order_id = v.order_id AND oi.qty <= 0);";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "SELECT *",
        "FROM asdf",
        "WHERE EXISTS (",
        "        SELECT 1",
        "        FROM oqt_t_order_item oi",
        "        WHERE oi.order_id = v.order_id",
        "            AND oi.sku LIKE 'SKU-%'",
        "    )",
        "    AND NOT EXISTS (",
        "        SELECT 1",
        "        FROM oqt_t_order_item oi",
        "        WHERE oi.order_id = v.order_id",
        "            AND oi.qty <= 0",
        "    );",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn compute_incremental_start_rewinds_to_current_line_start() {
    let text = "SELECT column_name FROM dual";
    let pos = text.find("name").unwrap_or(0) as i32;
    let start = compute_incremental_start_from_text(text, pos, 1, 0);
    let expected = 0;
    assert_eq!(start, expected);
}

#[test]
fn compute_incremental_start_clamps_to_utf8_boundary() {
    let text = "SELECT 한글컬럼 FROM dual";
    let base = text.find("글").unwrap_or(0);
    let mid_byte = base.saturating_add(1) as i32;
    let start = compute_incremental_start_from_text(text, mid_byte, 1, 0);
    assert!(text.is_char_boundary(start));
    assert!(start <= base);
}

#[test]
fn is_string_or_comment_style_matches_only_multiline_continuations() {
    assert!(!is_string_or_comment_style(STYLE_COMMENT));
    assert!(is_string_or_comment_style(STYLE_STRING));
    assert!(!is_string_or_comment_style(STYLE_DEFAULT));
    assert!(!is_string_or_comment_style(STYLE_KEYWORD));
}

#[test]
fn incremental_highlighting_matches_full_styles_after_inserting_block_comment() {
    let original = "SELECT 1\nvalue\nSELECT 2";
    let insert_pos = original.find("value").unwrap_or(0);
    let updated = format!(
        "{}/* {}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 3, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_deleting_block_comment() {
    let original = "SELECT 1\n/* value\nSELECT 2";
    let delete_pos = original.find("/* ").unwrap_or(0);
    let updated = format!(
        "{}{}",
        original.get(..delete_pos).unwrap_or(""),
        original.get(delete_pos.saturating_add(3)..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, delete_pos, 0, 3)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_inserting_q_quote_prefix() {
    let original = "SELECT body\nline]'\nFROM dual";
    let insert_pos = original.find("body").unwrap_or(0);
    let updated = format!(
        "{}q'[{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 3, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_inserting_single_quote() {
    let original = "SELECT value\nFROM dual";
    let insert_pos = original.find("value").unwrap_or(0);
    let updated = format!(
        "{}'{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 1, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_inserting_line_comment_prefix() {
    let original = "SELECT 1
value
SELECT 2";
    let insert_pos = original.find("value").unwrap_or(0);
    let updated = format!(
        "{}-- {}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 3, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_deleting_line_comment_prefix() {
    let original = "SELECT 1
-- value
SELECT 2";
    let delete_pos = original.find("-- ").unwrap_or(0);
    let updated = format!(
        "{}{}",
        original.get(..delete_pos).unwrap_or(""),
        original.get(delete_pos.saturating_add(3)..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, delete_pos, 0, 3)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_rehighlight_start_does_not_rewind_previous_line_on_newline_edit() {
    let original = "SELECT 1\nWHERE col = 1\nORDER BY 1";
    let delete_pos = original.find("\nWHERE").unwrap_or(0);
    let updated = format!(
        "{}{}",
        original.get(..delete_pos).unwrap_or(""),
        original.get(delete_pos.saturating_add(1)..).unwrap_or("")
    );
    let expected = line_start_for_text(&updated, delete_pos);

    assert_eq!(
        incremental_rehighlight_start_for_text(&updated, delete_pos),
        expected
    );
}

#[test]
fn incremental_highlighting_matches_full_styles_after_crlf_block_comment_insert() {
    let original = "SELECT 1\r\nvalue\r\nSELECT 2";
    let insert_pos = original.find("value").unwrap_or(0);
    let updated = format!(
        "{}/* {}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 3, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_crlf_single_quote_insert() {
    let original = "SELECT value\r\nFROM dual";
    let insert_pos = original.find("value").unwrap_or(0);
    let updated = format!(
        "{}'{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 1, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_inside_continued_block_comment_line() {
    let original = "SELECT 1\n/* block\ncomment line\n*/\nSELECT 2";
    let target = "comment";
    let insert_pos = original.find(target).unwrap_or(0);
    let updated = format!(
        "{}x{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 1, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_inside_continued_single_quote_line() {
    let original = "SELECT 'first line\nsecond line'\nFROM dual";
    let insert_pos = original.find("second").unwrap_or(0);
    let updated = format!(
        "{}x{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 1, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_inside_continued_q_quote_line() {
    let original = "SELECT q'[first line\nsecond line]'\nFROM dual";
    let insert_pos = original.find("second").unwrap_or(0);
    let updated = format!(
        "{}x{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 1, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn incremental_highlighting_matches_full_styles_after_inserting_prompt_prefix() {
    let original = "value\nSELECT 1";
    let insert_pos = 0usize;
    let updated = format!("PROMPT {}", original);

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 7, 0)
        .unwrap_or_default();
    let full = SqlHighlighter::new().generate_styles_for_text(&updated);

    assert_eq!(incremental, full);
}

#[test]
fn encode_fltk_style_bytes_zeroes_utf8_continuations() {
    let text = "가A";
    let logical = std::iter::repeat_n(STYLE_KEYWORD, text.len()).collect::<String>();
    let encoded = encode_fltk_style_bytes(text, &logical).unwrap_or_default();

    assert_eq!(encoded.len(), text.len());
    assert_eq!(encoded.first().copied(), Some(STYLE_KEYWORD as u8));
    assert_eq!(encoded.get(1).copied(), Some(0));
    assert_eq!(encoded.get(2).copied(), Some(0));
    assert_eq!(encoded.get(3).copied(), Some(STYLE_KEYWORD as u8));
}

#[test]
fn encode_fltk_style_bytes_ascii_fast_path_preserves_bytes() {
    let text = "SELECT col FROM dual";
    let logical = std::iter::repeat_n(STYLE_KEYWORD, text.len()).collect::<String>();
    let encoded = encode_fltk_style_bytes(text, &logical).unwrap_or_default();

    assert_eq!(encoded, logical.into_bytes());
    assert_eq!(
        encode_repeated_fltk_style_bytes(text, STYLE_COMMENT),
        vec![STYLE_COMMENT as u8; text.len()]
    );
}

#[test]
fn encoded_style_bytes_preserve_utf8_byte_length_after_multibyte_edit() {
    let original = "SELECT '가'";
    let insert_pos = original.find('가').unwrap_or(0);
    let updated = format!(
        "{}나다{}",
        original.get(..insert_pos).unwrap_or(""),
        original.get(insert_pos..).unwrap_or("")
    );

    let incremental = apply_incremental_highlight_for_test(original, &updated, insert_pos, 6, 0)
        .unwrap_or_default();
    let encoded = encode_fltk_style_bytes(&updated, &incremental).unwrap_or_default();

    assert_eq!(incremental.len(), updated.len());
    assert_eq!(encoded.len(), updated.len());
    assert_eq!(
        encoded.get(insert_pos).copied(),
        incremental.as_bytes().get(insert_pos).copied()
    );
    assert_eq!(encoded.get(insert_pos.saturating_add(1)).copied(), Some(0));
    assert_eq!(encoded.get(insert_pos.saturating_add(2)).copied(), Some(0));
}

#[test]
fn format_sql_uses_parser_depth_for_plsql_blocks() {
    let input = r#"BEGIN
IF 1 = 1 THEN
BEGIN
NULL;
END;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF 1 = 1 THEN",
        "        BEGIN",
        "            NULL;",
        "        END;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_pre_dedents_else_elsif_exception_lines() {
    let input = r#"BEGIN
IF v_flag = 'Y' THEN
NULL;
ELSIF v_flag = 'N' THEN
NULL;
ELSE
NULL;
END IF;
EXCEPTION
WHEN OTHERS THEN
NULL;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF v_flag = 'Y' THEN",
        "        NULL;",
        "    ELSIF v_flag = 'N' THEN",
        "        NULL;",
        "    ELSE",
        "        NULL;",
        "    END IF;",
        "EXCEPTION",
        "    WHEN OTHERS THEN",
        "        NULL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_parser_depth_indents_if_and_case_one_level_more() {
    let input = r#"BEGIN
IF v_flag = 'Y' THEN
CASE
WHEN v_num = 1 THEN
NULL;
ELSE
NULL;
END CASE;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF v_flag = 'Y' THEN",
        "        CASE",
        "            WHEN v_num = 1 THEN",
        "                NULL;",
        "            ELSE",
        "                NULL;",
        "        END CASE;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_case_branches_with_blank_lines() {
    let input = r#"BEGIN
CASE
WHEN p_n < 0 THEN
v := p_n * p_n;
WHEN p_n BETWEEN 0 AND 10 THEN
x := p_n + 100;
v := x - 50;
ELSE
v := p_n + 999;
END CASE;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    CASE",
        "        WHEN p_n < 0 THEN",
        "            v := p_n * p_n;",
        "        WHEN p_n BETWEEN 0 AND 10 THEN",
        "            x := p_n + 100;",
        "            v := x - 50;",
        "        ELSE",
        "            v := p_n + 999;",
        "    END CASE;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_comments_together() {
    let input = r#"BEGIN
-- first
-- second
NULL;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    -- first",
        "    -- second",
        "    NULL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_does_not_insert_blank_line_between_line_comments() {
    let input = "-- first\n-- second\nSELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = ["-- first", "-- second", "", "SELECT 1", "FROM DUAL;"].join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_consecutive_sqlplus_comments_together() {
    let input = "REM first\nREMARK second\nSELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = ["REM first", "REMARK second", "", "SELECT 1", "FROM DUAL;"].join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_statement_boundary_when_semicolon_has_trailing_line_comment() {
    let input = "SELECT 1 FROM dual; -- trailing note\nSELECT 2 FROM dual;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT 1\nFROM DUAL;"),
        "first statement should remain independent, got: {formatted}"
    );
    assert!(
        formatted.contains("SELECT 2\nFROM DUAL;"),
        "second statement should remain independent, got: {formatted}"
    );
}

#[test]
fn format_sql_does_not_insert_blank_line_between_prompt_commands() {
    let input = "PROMPT one\nPROMPT two\nSELECT 1 FROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = ["PROMPT one", "PROMPT two", "", "SELECT 1", "FROM DUAL;"].join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_clear_breaks_and_computes_on_separate_lines() {
    let input = "CLEAR BREAKS\nCLEAR COMPUTES;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = "CLEAR BREAKS\nCLEAR COMPUTES";

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_splits_combined_clear_breaks_and_computes() {
    let input = "CLEAR BREAKS CLEAR COMPUTES;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = "CLEAR BREAKS\nCLEAR COMPUTES";

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_indents_line_comments_to_depth() {
    let input = r#"BEGIN
IF 1 = 1 THEN
-- inside if
NULL;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF 1 = 1 THEN",
        "        -- inside if",
        "        NULL;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_preserves_multiline_block_comment_internal_indentation() {
    let input = r#"BEGIN
IF 1 = 1 THEN
/* block comment
still block comment */
NULL;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF 1 = 1 THEN",
        "        /* block comment",
        "still block comment */",
        "        NULL;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_plsql_depth_overrides_manual_overindent_for_code_lines() {
    let input = r#"BEGIN
                    IF 1 = 1 THEN
                                NULL;
                    END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF 1 = 1 THEN",
        "        NULL;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_plsql_depth_overrides_manual_overindent_for_comment_lines() {
    let input = r#"BEGIN
                    IF 1 = 1 THEN
                                -- deeply indented comment
                                NULL;
                    END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    IF 1 = 1 THEN",
        "        -- deeply indented comment",
        "        NULL;",
        "    END IF;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_plsql_depth_overrides_manual_overindent_for_dml_lines() {
    let input = r#"BEGIN
                    SELECT
                                col1
                            INTO
                                            v_col1
                                FROM dual;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT col1",
        "    INTO v_col1",
        "    FROM DUAL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_plsql_depth_overrides_manual_overindent_for_dml_comment_lines() {
    let input = r#"BEGIN
                    SELECT
                                col1
                            INTO
                                            -- selected value comment
                                            v_col1
                                FROM dual;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "BEGIN",
        "    SELECT col1",
        "    INTO",
        "        -- selected value comment",
        "        v_col1",
        "    FROM DUAL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_end_if_depth_before_named_end_when_line_comment_in_between() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
PROCEDURE p IS
BEGIN
IF 1 = 1 THEN
NULL;
END IF;
-- keep pending end label scope
END p;
END demo_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "CREATE OR REPLACE PACKAGE BODY demo_pkg AS",
        "    PROCEDURE p IS",
        "    BEGIN",
        "        IF 1 = 1 THEN",
        "            NULL;",
        "        END IF;",
        "        -- keep pending end label scope",
        "    END p;",
        "END demo_pkg;",
        "/",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_end_if_depth_before_named_end_when_block_comment_in_between() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
PROCEDURE p IS
BEGIN
IF 1 = 1 THEN
NULL;
END IF;
/* keep pending end label scope */
END p;
END demo_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "CREATE OR REPLACE PACKAGE BODY demo_pkg AS",
        "    PROCEDURE p IS",
        "    BEGIN",
        "        IF 1 = 1 THEN",
        "            NULL;",
        "        END IF;",
        "        /* keep pending end label scope */",
        "    END p;",
        "END demo_pkg;",
        "/",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_end_if_depth_before_named_end_when_multiline_block_comment_in_between() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
PROCEDURE p IS
BEGIN
IF 1 = 1 THEN
NULL;
END IF;
/* keep pending
end label scope */
END p;
END demo_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "CREATE OR REPLACE PACKAGE BODY demo_pkg AS",
        "    PROCEDURE p IS",
        "    BEGIN",
        "        IF 1 = 1 THEN",
        "            NULL;",
        "        END IF;",
        "        /* keep pending",
        "end label scope */",
        "    END p;",
        "END demo_pkg;",
        "/",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_keeps_end_if_depth_before_named_end_when_sqlplus_comment_and_block_comment_in_between(
) {
    let input = r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
PROCEDURE p IS
BEGIN
IF 1 = 1 THEN
NULL;
END IF;
-- keep pending end label scope
/* still in same scope */
END p;
END demo_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "CREATE OR REPLACE PACKAGE BODY demo_pkg AS",
        "    PROCEDURE p IS",
        "    BEGIN",
        "        IF 1 = 1 THEN",
        "            NULL;",
        "        END IF;",
        "        -- keep pending end label scope",
        "        /* still in same scope */",
        "    END p;",
        "END demo_pkg;",
        "/",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_select_case_inside_sum_is_indented() {
    let input = r#"SELECT grp,
COUNT(*) AS cnt,
SUM (
CASE
WHEN MOD (n, 2) = 0 THEN 1
ELSE 0
END) AS even_cnt,
SUM (
CASE
WHEN INSTR (txt, 'END;') > 0 THEN 1
ELSE 0
END) AS has_end_token_cnt
FROM oqt_t_test
GROUP BY grp
ORDER BY grp;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "SELECT grp,",
        "    COUNT(*) AS cnt,",
        "    SUM (",
        "        CASE",
        "            WHEN MOD (n, 2) = 0 THEN 1",
        "            ELSE 0",
        "        END",
        "    ) AS even_cnt,",
        "    SUM (",
        "        CASE",
        "            WHEN INSTR (txt, 'END;') > 0 THEN 1",
        "            ELSE 0",
        "        END",
        "    ) AS has_end_token_cnt",
        "FROM oqt_t_test",
        "GROUP BY grp",
        "ORDER BY grp;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_nested_case_expression_in_select_keeps_newlines() {
    let input = r#"SELECT
CASE
WHEN a = 1 THEN CASE WHEN b = 2 THEN 'X' ELSE 'Y' END
ELSE CASE WHEN c = 3 THEN 'Z' ELSE 'W' END
END AS result_value,
col2
FROM dual;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "SELECT",
        "    CASE",
        "        WHEN a = 1 THEN",
        "            CASE",
        "                WHEN b = 2 THEN 'X'",
        "                ELSE 'Y'",
        "            END",
        "        ELSE",
        "            CASE",
        "                WHEN c = 3 THEN 'Z'",
        "                ELSE 'W'",
        "            END",
        "    END AS result_value,",
        "    col2",
        "FROM DUAL;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_boss7_extreme_scalar_subqueries_nested_case_keeps_depth() {
    let input = r#"/*===========================================================================
  BOSS 7
  Extreme scalar subqueries + nested CASE + LISTAGG + analytic in inline views
===========================================================================*/
WITH
    t AS
    (
        SELECT 1 AS grp_id, 'A' AS code, 10 AS val FROM dual
        UNION ALL
        SELECT 1, 'B', 20 FROM dual
        UNION ALL
        SELECT 1, 'C', 30 FROM dual
        UNION ALL
        SELECT 2, 'A', 5  FROM dual
        UNION ALL
        SELECT 2, 'B', 15 FROM dual
        UNION ALL
        SELECT 2, 'C', 25 FROM dual
    )
SELECT
    x.grp_id,
    x.code,
    x.val,
    (
        SELECT LISTAGG(y.code || ':' || y.val, ',')
               WITHIN GROUP (ORDER BY y.val DESC, y.code)
        FROM t y
        WHERE y.grp_id = x.grp_id
    ) AS grp_summary,
    (
        SELECT MAX(z.val)
        FROM
        (
            SELECT
                t2.*,
                DENSE_RANK() OVER (PARTITION BY t2.grp_id ORDER BY t2.val DESC, t2.code) AS dr
            FROM t t2
            WHERE t2.grp_id = x.grp_id
        ) z
        WHERE z.dr = 1
    ) AS grp_top_val,
    CASE
        WHEN x.val =
             (
                 SELECT MAX(m.val)
                 FROM t m
                 WHERE m.grp_id = x.grp_id
             )
        THEN
            CASE
                WHEN x.code =
                     (
                         SELECT MIN(n.code) KEEP (DENSE_RANK FIRST ORDER BY n.val DESC, n.code)
                         FROM t n
                         WHERE n.grp_id = x.grp_id
                     )
                THEN 'TOP_AND_FIRST_CODE'
                ELSE 'TOP_BUT_NOT_FIRST_CODE'
            END
        ELSE
            CASE
                WHEN x.val >
                     (
                         SELECT AVG(a.val)
                         FROM t a
                         WHERE a.grp_id = x.grp_id
                     )
                THEN 'ABOVE_AVG'
                ELSE 'NOT_ABOVE_AVG'
            END
    END AS class_flag
FROM t x
ORDER BY
    x.grp_id,
    x.code;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let inline_from_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FROM ("))
        .unwrap_or(0);
    let inline_select_idx = lines
        .iter()
        .enumerate()
        .skip(inline_from_idx.saturating_add(1))
        .find_map(|(idx, line)| (line.trim_start() == "SELECT").then_some(idx))
        .unwrap_or(0);

    let inline_from_indent = lines[inline_from_idx]
        .len()
        .saturating_sub(lines[inline_from_idx].trim_start().len());
    let inline_select_indent = lines[inline_select_idx]
        .len()
        .saturating_sub(lines[inline_select_idx].trim_start().len());

    assert!(
        inline_select_indent > inline_from_indent,
        "inline-view SELECT should be indented deeper than FROM (, got:\n{}",
        formatted
    );

    let case_idx = lines
        .iter()
        .position(|line| line.trim_start() == "CASE")
        .unwrap_or(0);
    let when_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHEN x.val = ("))
        .unwrap_or(0);
    let inner_case_idx = lines
        .iter()
        .enumerate()
        .skip(case_idx.saturating_add(1))
        .find_map(|(idx, line)| (line.trim_start() == "CASE").then_some(idx))
        .unwrap_or(0);

    let case_indent = lines[case_idx]
        .len()
        .saturating_sub(lines[case_idx].trim_start().len());
    let when_indent = lines[when_idx]
        .len()
        .saturating_sub(lines[when_idx].trim_start().len());
    let inner_case_indent = lines[inner_case_idx]
        .len()
        .saturating_sub(lines[inner_case_idx].trim_start().len());

    assert!(
        when_indent > case_indent,
        "WHEN branch should be indented under CASE, got:\n{}",
        formatted
    );
    assert!(
        inner_case_indent >= when_indent,
        "nested CASE should not outdent before parent WHEN depth, got:\n{}",
        formatted
    );

    assert!(
        formatted.contains("WITH t AS ("),
        "CTE header should stay intact, got:\n{}",
        formatted
    );
    assert!(
        formatted.contains("ORDER BY x.grp_id,"),
        "ORDER BY should remain attached to first sort key line, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_package_body_with_nested_case_keeps_block_newlines() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg_case AS PROCEDURE run_demo IS BEGIN CASE v_mode WHEN 1 THEN CASE WHEN v_flag = 'Y' THEN NULL; ELSE NULL; END CASE; ELSE NULL; END CASE; END run_demo; END pkg_case;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "CASE v_mode
            WHEN 1 THEN
                CASE
                    WHEN v_flag = 'Y' THEN"
        ),
        "Nested CASE in package body should keep multi-line layout, got: {}",
        formatted
    );
    assert!(
        formatted.contains(
            "END CASE;
            ELSE
                NULL;
        END CASE;"
        ),
        "Outer CASE branches should remain separated by new lines, got: {}",
        formatted
    );
}

#[test]
fn format_sql_package_body_case_inside_parentheses_keeps_newlines() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg_case_paren AS PROCEDURE run_demo IS v_val NUMBER; BEGIN v_val := fn_calc((CASE WHEN v_mode = 1 THEN CASE WHEN v_flag = 'Y' THEN 100 ELSE 200 END ELSE 0 END)); END run_demo; END pkg_case_paren;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "v_val := fn_calc ((\n                CASE\n                    WHEN v_mode = 1 THEN"
        ),
        "CASE expression inside (( should expand with progressive depth, got: {}",
        formatted
    );
    assert!(
        formatted.contains(
            "WHEN v_flag = 'Y' THEN\n                                100\n                            ELSE\n                                200\n                        END"
        ),
        "Nested CASE branches inside parenthesis should stay on separate lines, got: {}",
        formatted
    );
}

#[test]
fn format_sql_package_body_type_table_is_not_misdetected_as_create_table() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg_case_type AS TYPE num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER; FUNCTION run_demo RETURN NUMBER IS v_out NUMBER; BEGIN v_out := fn_calc((CASE WHEN v_mode = 1 THEN CASE WHEN v_flag = 'Y' THEN 10 ELSE 20 END ELSE 0 END)); RETURN v_out; END run_demo; END pkg_case_type;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.starts_with("CREATE OR REPLACE PACKAGE BODY"),
        "Package body should keep CREATE PACKAGE prefix, got: {}",
        formatted
    );
    assert!(
        formatted.contains("TYPE num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;"),
        "TYPE ... IS TABLE declaration should be preserved, got: {}",
        formatted
    );
    assert!(
        formatted.contains("BEGIN\n        v_out := fn_calc ((\n                CASE"),
        "Nested CASE inside function body with (( should use progressive depth, got: {}",
        formatted
    );
}

#[test]
fn format_sql_package_body_type_table_with_nested_case_keeps_newlines() {
    let input = "CREATE OR REPLACE PACKAGE BODY pkg_case_type AS TYPE num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER; FUNCTION run_demo RETURN NUMBER IS BEGIN CASE WHEN v_mode = 1 THEN CASE WHEN v_flag = 'Y' THEN 10 ELSE 20 END ELSE 0 END CASE; RETURN 1; END run_demo; END pkg_case_type;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "TYPE num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;\n    FUNCTION run_demo RETURN NUMBER IS"
        ),
        "TYPE ... IS TABLE declaration should not collapse following routine block, got: {}",
        formatted
    );
    assert!(
        formatted.contains(
            "BEGIN
        CASE
            WHEN v_mode = 1 THEN
                CASE
                    WHEN v_flag = 'Y' THEN"
        ),
        "Nested CASE after TYPE ... IS TABLE should remain multiline, got: {}",
        formatted
    );
}

#[test]
fn format_sql_package_body_complex_nested_blocks_keeps_following_member_in_body() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY test_pkg
IS

    ----------------------------------------------------------------
    -- nested function
    ----------------------------------------------------------------
    FUNCTION calc_bonus(
        p_salary NUMBER,
        p_grade  VARCHAR2
    ) RETURN NUMBER
    IS
        v_bonus NUMBER := 0;
    BEGIN

        CASE p_grade
            WHEN 'A' THEN
                v_bonus := p_salary * 0.30;
            WHEN 'B' THEN
                v_bonus := p_salary * 0.20;
            WHEN 'C' THEN
                v_bonus := p_salary * 0.10;
            ELSE
                v_bonus := 0;
        END CASE;

        RETURN v_bonus;

    END calc_bonus;

    ----------------------------------------------------------------
    -- procedure with complex nesting
    ----------------------------------------------------------------
    PROCEDURE process_emp(
        p_deptno NUMBER
    )
    IS

        CURSOR c_emp IS
            SELECT empno, ename, sal
            FROM emp
            WHERE deptno = p_deptno;

        v_sql       VARCHAR2(4000);
        v_bonus     NUMBER;
        v_total     NUMBER := 0;

    BEGIN

        FOR r IN c_emp
        LOOP

            BEGIN

                v_bonus := calc_bonus(
                    r.sal,
                    CASE
                        WHEN r.sal > 5000 THEN 'A'
                        WHEN r.sal > 3000 THEN 'B'
                        ELSE 'C'
                    END
                );

                IF v_bonus > 0 THEN

                    FOR i IN 1 .. 3
                    LOOP

                        v_total := v_total + (v_bonus * i);

                        IF MOD(i,2) = 0 THEN
                            DBMS_OUTPUT.PUT_LINE(
                                'EMP=' || r.empno
                                || ' BONUS=' || v_bonus
                                || ' ITER=' || i
                            );
                        ELSE

                            CASE
                                WHEN i = 1 THEN
                                    NULL;
                                WHEN i = 3 THEN
                                    DBMS_OUTPUT.PUT_LINE('FINAL ITERATION');
                                ELSE
                                    NULL;
                            END CASE;

                        END IF;

                    END LOOP;

                ELSE
                    DBMS_OUTPUT.PUT_LINE('NO BONUS');
                END IF;

            EXCEPTION
                WHEN OTHERS THEN
                    DBMS_OUTPUT.PUT_LINE(
                        'ERROR:' || SQLERRM
                    );
            END;

        END LOOP;

        ----------------------------------------------------------------
        -- dynamic sql block
        ----------------------------------------------------------------
        BEGIN

            v_sql := q'[
                INSERT INTO bonus_log(emp_count,total_bonus)
                SELECT COUNT(*), :1
                FROM emp
                WHERE deptno = :2
            ]';

            EXECUTE IMMEDIATE v_sql
                USING v_total, p_deptno;

        EXCEPTION
            WHEN OTHERS THEN
                DBMS_OUTPUT.PUT_LINE('LOG ERROR');
        END;

    END process_emp;

    ----------------------------------------------------------------
    -- nested block test
    ----------------------------------------------------------------
    PROCEDURE nested_block_test
    IS
        v_cnt NUMBER := 0;
    BEGIN

        DECLARE
            v_inner NUMBER := 10;
        BEGIN

            WHILE v_inner > 0
            LOOP

                BEGIN

                    v_cnt := v_cnt + 1;

                    IF v_cnt > 5 THEN
                        EXIT;
                    END IF;

                END;

                v_inner := v_inner - 1;

            END LOOP;

        END;

        DBMS_OUTPUT.PUT_LINE('COUNT=' || v_cnt);

    END nested_block_test;

END test_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "    END process_emp;\n\n    ----------------------------------------------------------------\n    -- nested block test"
        ),
        "following package member comments should stay inside package body depth, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "    PROCEDURE nested_block_test IS\n        v_cnt NUMBER := 0;\n    BEGIN"
        ),
        "following package member declaration should not split away from its BEGIN block, got: {formatted}"
    );
    assert!(
        formatted.contains("        DECLARE\n            v_inner NUMBER := 10;\n        BEGIN"),
        "nested DECLARE block should keep procedure-body indentation, got: {formatted}"
    );
    assert!(
        formatted.contains("    END nested_block_test;\nEND test_pkg;\n/"),
        "package body should close only after the last nested member, got: {formatted}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(formatted, formatted_again);
}

#[test]
fn format_sql_nested_package_script_keeps_body_initializer_and_following_block() {
    let input = r#"CREATE OR REPLACE PACKAGE fmt_nested_pkg AS
    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;

    PROCEDURE run_demo(
        p_seed   IN NUMBER DEFAULT 3,
        p_result OUT CLOB
    );

    FUNCTION calc_value(
        p_base IN NUMBER,
        p_mode IN VARCHAR2 DEFAULT 'NORMAL'
    ) RETURN NUMBER;
END fmt_nested_pkg;
/
CREATE OR REPLACE PACKAGE BODY fmt_nested_pkg AS
    c_limit CONSTANT PLS_INTEGER := 7;
    g_state VARCHAR2(30) := 'INIT';

    TYPE t_row IS RECORD (
        id   NUMBER,
        txt  VARCHAR2(100),
        amt  NUMBER,
        flag VARCHAR2(1)
    );

    TYPE t_row_tab IS TABLE OF t_row INDEX BY PLS_INTEGER;

    PROCEDURE append_text(
        io_text IN OUT NOCOPY CLOB,
        p_piece IN VARCHAR2
    ) IS
    BEGIN
        io_text := io_text || p_piece || CHR(10);
    END append_text;

    FUNCTION calc_value(
        p_base IN NUMBER,
        p_mode IN VARCHAR2 DEFAULT 'NORMAL'
    ) RETURN NUMBER IS
        l_result NUMBER := NVL(p_base, 0);
        l_factor NUMBER := 1;

        FUNCTION inner_adjust(
            p_input IN NUMBER
        ) RETURN NUMBER IS
            l_tmp NUMBER := NVL(p_input, 0);
        BEGIN
            FOR i IN 1 .. 3 LOOP
                l_tmp :=
                    CASE
                        WHEN MOD(i, 2) = 0 THEN l_tmp + 5
                        ELSE l_tmp + 2
                    END;
            END LOOP;

            RETURN l_tmp;
        END inner_adjust;
    BEGIN
        l_factor :=
            CASE UPPER(TRIM(p_mode))
                WHEN 'HIGH'   THEN 3
                WHEN 'MEDIUM' THEN 2
                WHEN 'LOW'    THEN 1
                ELSE 1
            END;

        l_result := inner_adjust(l_result) * l_factor;

        <<validation_block>>
        BEGIN
            IF l_result > 100 THEN
                l_result := ROUND(l_result / 2, 2);
            ELSIF l_result BETWEEN 50 AND 100 THEN
                l_result := ROUND(l_result * 1.1, 2);
            ELSE
                l_result := ROUND(l_result + 7, 2);
            END IF;
        EXCEPTION
            WHEN VALUE_ERROR THEN
                l_result := -1;
        END validation_block;

        RETURN l_result;
    EXCEPTION
        WHEN OTHERS THEN
            RETURN -9999;
    END calc_value;

    PROCEDURE run_demo(
        p_seed   IN NUMBER DEFAULT 3,
        p_result OUT CLOB
    ) IS
        l_rows      t_row_tab;
        l_idx       PLS_INTEGER := 0;
        l_total     NUMBER := 0;
        l_count     NUMBER := 0;
        l_status    VARCHAR2(30);
        l_sql       VARCHAR2(4000);
        l_json_like VARCHAR2(4000);
        l_mode      VARCHAR2(10);

        CURSOR c_data(cp_seed NUMBER) IS
            SELECT LEVEL AS id,
                   'ITEM_' || TO_CHAR(LEVEL) AS txt,
                   cp_seed * LEVEL AS amt,
                   CASE
                       WHEN MOD(LEVEL, 2) = 0 THEN 'Y'
                       ELSE 'N'
                   END AS flag
              FROM dual
           CONNECT BY LEVEL <= LEAST(GREATEST(cp_seed, 1), 6);

        PROCEDURE process_row(
            p_row   IN t_row,
            p_depth IN PLS_INTEGER DEFAULT 1
        ) IS
            l_local NUMBER := 0;

            PROCEDURE nested_walk(
                p_start IN PLS_INTEGER
            ) IS
                l_step PLS_INTEGER := p_start;
            BEGIN
                WHILE l_step <= 3 LOOP
                    l_local := l_local +
                        CASE
                            WHEN p_row.flag = 'Y' AND l_step = 1 THEN 100
                            WHEN p_row.flag = 'Y' THEN 10 * l_step
                            WHEN p_row.flag = 'N' AND l_step = 3 THEN 3
                            ELSE l_step
                        END;

                    l_step := l_step + 1;
                END LOOP;
            END nested_walk;
        BEGIN
            IF p_depth <= 2 THEN
                nested_walk(1);
            ELSE
                l_local := -1;
            END IF;

            FOR j IN REVERSE 1 .. 2 LOOP
                BEGIN
                    IF j = 2 THEN
                        l_local := l_local + calc_value(p_row.amt, 'HIGH');
                    ELSE
                        l_local := l_local +
                            CASE
                                WHEN p_row.amt > 10 THEN calc_value(p_row.amt, 'MEDIUM')
                                ELSE calc_value(p_row.amt, 'LOW')
                            END;
                    END IF;
                EXCEPTION
                    WHEN OTHERS THEN
                        l_local := l_local - 50;
                END;
            END LOOP;

            l_total := l_total + l_local;
        EXCEPTION
            WHEN OTHERS THEN
                l_total := l_total - 999;
        END process_row;
    BEGIN
        p_result := TO_CLOB('');

        l_json_like := q'!{
  "check": "formatter",
  "text": "package body / nested begin-end / case / loop / dynamic sql"
}!';

        append_text(p_result, 'START');
        append_text(p_result, 'STATE=' || g_state);
        append_text(p_result, 'RAW=' || REPLACE(l_json_like, CHR(10), ' '));

        FOR r IN c_data(p_seed) LOOP
            l_idx := l_idx + 1;

            l_rows(l_idx).id   := r.id;
            l_rows(l_idx).txt  := r.txt;
            l_rows(l_idx).amt  := r.amt;
            l_rows(l_idx).flag := r.flag;

            process_row(
                l_rows(l_idx),
                CASE
                    WHEN MOD(r.id, 2) = 0 THEN 2
                    ELSE 1
                END
            );

            EXIT WHEN l_idx >= c_limit AND p_seed = 9999;
        END LOOP;

        BEGIN
            l_sql := q'[select count(*) from dual connect by level <= :x]';

            EXECUTE IMMEDIATE l_sql
                INTO l_count
                USING LEAST(GREATEST(p_seed, 1), 4);

            CASE
                WHEN l_count = 0 THEN
                    l_status := 'EMPTY';
                WHEN l_count BETWEEN 1 AND 2 THEN
                    l_status := 'SMALL';
                WHEN l_count BETWEEN 3 AND 4 THEN
                    l_status := 'MEDIUM';
                ELSE
                    l_status := 'LARGE';
            END CASE;
        EXCEPTION
            WHEN OTHERS THEN
                l_status := 'DYN_SQL_ERROR';
        END;

        FOR i IN 1 .. l_rows.COUNT LOOP
            CONTINUE WHEN l_rows.EXISTS(i)
                      AND l_rows(i).flag = 'N'
                      AND l_rows(i).amt < 5;

            l_mode :=
                CASE
                    WHEN l_rows(i).amt >= 12 THEN 'HIGH'
                    WHEN l_rows(i).amt >= 6 THEN 'MEDIUM'
                    ELSE 'LOW'
                END;

            append_text(
                p_result,
                '[' || i || '] '
                || l_rows(i).txt
                || ' / mode=' || l_mode
                || ' / calc=' || TO_CHAR(calc_value(l_rows(i).amt, l_mode))
            );
        END LOOP;

        append_text(p_result, 'STATUS=' || l_status);
        append_text(p_result, 'TOTAL=' || TO_CHAR(l_total));

        <<final_block>>
        BEGIN
            IF l_total > 500 THEN
                append_text(p_result, 'FINAL=VERY_HIGH');
            ELSIF l_total > 200 THEN
                append_text(p_result, 'FINAL=HIGH');
            ELSIF l_total > 100 THEN
                append_text(p_result, 'FINAL=MID');
            ELSE
                append_text(p_result, 'FINAL=LOW');
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                append_text(p_result, 'FINAL=ERROR');
        END final_block;
    EXCEPTION
        WHEN OTHERS THEN
            p_result := 'RUN_DEMO_ERROR: ' || SQLERRM;
    END run_demo;

BEGIN
    g_state :=
        CASE
            WHEN g_state IS NULL THEN 'BOOT'
            ELSE g_state || '_READY'
        END;
END fmt_nested_pkg;
/
DECLARE
    l_result CLOB;
BEGIN
    fmt_nested_pkg.run_demo(4, l_result);
    DBMS_OUTPUT.PUT_LINE(DBMS_LOB.SUBSTR(l_result, 32767, 1));
END;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted
            .contains("END fmt_nested_pkg;\n/\n\nCREATE OR REPLACE PACKAGE BODY fmt_nested_pkg AS"),
        "package spec/body separator should stay intact, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "FUNCTION inner_adjust (p_input IN NUMBER) RETURN NUMBER IS\n            l_tmp NUMBER := NVL (p_input, 0);\n        BEGIN"
        ),
        "nested local function should stay inside calc_value declaration depth, got: {formatted}"
    );
    assert!(
        formatted
            .contains("<<validation_block>>\n        BEGIN\n            IF l_result > 100 THEN"),
        "labeled validation block should keep nested BEGIN depth, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "PROCEDURE process_row (p_row IN t_row, p_depth IN PLS_INTEGER DEFAULT 1) IS\n            l_local NUMBER := 0;\n            PROCEDURE nested_walk (p_start IN PLS_INTEGER) IS"
        ),
        "nested local procedures should remain within run_demo declaration section, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "BEGIN\n    g_state :=\n        CASE\n            WHEN g_state IS NULL THEN\n                'BOOT'\n            ELSE\n                g_state || '_READY'\n        END;\nEND fmt_nested_pkg;\n/\n\nDECLARE"
        ),
        "package body initializer should close on package END and preserve following anonymous block, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "DECLARE\n    l_result CLOB;\nBEGIN\n    fmt_nested_pkg.run_demo (4, l_result);\n    DBMS_OUTPUT.PUT_LINE (DBMS_LOB.SUBSTR (l_result, 32767, 1));\nEND;\n/"
        ),
        "trailing anonymous block should remain a separate formatted statement, got: {formatted}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(formatted, formatted_again);
}

#[test]
fn format_sql_torture_package_body_keeps_nested_blocks_and_labels() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY torture_pkg
IS

-------------------------------------------------------
-- TYPE 정의
-------------------------------------------------------

TYPE refcur IS REF CURSOR;

TYPE emp_rec IS RECORD
(
    empno   NUMBER,
    ename   VARCHAR2(100),
    sal     NUMBER
);

TYPE emp_tab IS TABLE OF emp_rec INDEX BY PLS_INTEGER;

-------------------------------------------------------
-- Autonomous Transaction Function
-------------------------------------------------------

FUNCTION log_message(p_msg VARCHAR2)
RETURN NUMBER
IS
PRAGMA AUTONOMOUS_TRANSACTION;

BEGIN

    INSERT INTO log_table(msg, log_time)
    VALUES(p_msg, SYSDATE);

    COMMIT;

    RETURN 1;

EXCEPTION
    WHEN OTHERS THEN
        ROLLBACK;
        RETURN -1;
END;

-------------------------------------------------------
-- Nested Logic Procedure
-------------------------------------------------------

PROCEDURE complex_logic(p_dept NUMBER)
IS

    v_tab      emp_tab;
    v_idx      NUMBER := 0;
    v_total    NUMBER := 0;

    CURSOR c_emp IS
        SELECT empno, ename, sal
        FROM emp
        WHERE deptno = p_dept;

BEGIN

    ---------------------------------------------------
    -- BULK COLLECT
    ---------------------------------------------------

    OPEN c_emp;

    LOOP
        FETCH c_emp BULK COLLECT INTO v_tab LIMIT 50;

        EXIT WHEN v_tab.COUNT = 0;

        <<outer_loop>>
        FOR i IN 1 .. v_tab.COUNT
        LOOP

            BEGIN

                v_idx := v_idx + 1;

                IF v_tab(i).sal > 5000 THEN

                    CASE
                        WHEN v_tab(i).sal > 10000 THEN
                            v_total := v_total + v_tab(i).sal * 0.5;

                        WHEN v_tab(i).sal > 7000 THEN
                            v_total := v_total + v_tab(i).sal * 0.3;

                        ELSE
                            v_total := v_total + v_tab(i).sal * 0.1;
                    END CASE;

                ELSE

                    DECLARE
                        v_inner NUMBER := 3;
                    BEGIN

                        WHILE v_inner > 0
                        LOOP

                            EXIT outer_loop WHEN v_inner = -1;

                            v_total := v_total + v_tab(i).sal;

                            v_inner := v_inner - 1;

                        END LOOP;

                    END;

                END IF;

            EXCEPTION
                WHEN ZERO_DIVIDE THEN
                    log_message('DIV ERROR');
                WHEN OTHERS THEN
                    log_message(SQLERRM);
            END;

        END LOOP;

    END LOOP;

    CLOSE c_emp;

END;

-------------------------------------------------------
-- Dynamic SQL + REF CURSOR
-------------------------------------------------------

PROCEDURE open_cursor(
    p_dept   NUMBER,
    p_cursor OUT refcur
)
IS

    v_sql VARCHAR2(4000);

BEGIN

    v_sql := q'[
        SELECT empno,
               ename,
               sal,
               CASE
                   WHEN sal > 5000 THEN 'HIGH'
                   WHEN sal > 3000 THEN 'MID'
                   ELSE 'LOW'
               END grade
        FROM emp
        WHERE deptno = :1
        ORDER BY sal DESC
    ]';

    OPEN p_cursor FOR v_sql USING p_dept;

END;

-------------------------------------------------------
-- FORALL + Exception Handling
-------------------------------------------------------

PROCEDURE bulk_raise_salary
IS

    TYPE id_tab IS TABLE OF NUMBER;
    v_ids id_tab := id_tab(7369, 7499, 7521, 7566);

BEGIN

    FORALL i IN 1 .. v_ids.COUNT SAVE EXCEPTIONS
        UPDATE emp
        SET sal = sal * 1.1
        WHERE empno = v_ids(i);

EXCEPTION

    WHEN OTHERS THEN

        FOR i IN 1 .. SQL%BULK_EXCEPTIONS.COUNT
        LOOP
            DBMS_OUTPUT.PUT_LINE(
                'ERROR INDEX=' || SQL%BULK_EXCEPTIONS(i).ERROR_INDEX ||
                ' CODE=' || SQL%BULK_EXCEPTIONS(i).ERROR_CODE
            );
        END LOOP;

END;

-------------------------------------------------------
-- Deep Nested Block
-------------------------------------------------------

PROCEDURE deep_nesting
IS

    v_counter NUMBER := 0;

BEGIN

    <<main_loop>>
    FOR i IN 1 .. 5
    LOOP

        DECLARE
            v_tmp NUMBER := i;
        BEGIN

            FOR j IN 1 .. 3
            LOOP

                IF j = 2 THEN

                    BEGIN

                        CASE
                            WHEN v_tmp = 1 THEN
                                v_counter := v_counter + 1;

                            WHEN v_tmp = 2 THEN
                                v_counter := v_counter + 2;

                            ELSE

                                DECLARE
                                    v_inner NUMBER := 5;
                                BEGIN

                                    LOOP
                                        EXIT WHEN v_inner = 0;

                                        v_counter := v_counter + v_inner;

                                        v_inner := v_inner - 1;
                                    END LOOP;

                                END;

                        END CASE;

                    END;

                ELSE
                    NULL;
                END IF;

            END LOOP;

        END;

    END LOOP;

    DBMS_OUTPUT.PUT_LINE('COUNTER=' || v_counter);

END;

END torture_pkg;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "FUNCTION log_message (p_msg VARCHAR2) RETURN NUMBER IS\n        PRAGMA AUTONOMOUS_TRANSACTION;\n    BEGIN"
        ),
        "autonomous transaction pragma should stay inside function declaration block, got: {formatted}"
    );
    assert!(
        formatted.contains("<<outer_loop>>\n            FOR i IN 1..v_tab.COUNT LOOP"),
        "outer loop label should stay attached to the nested FOR loop, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "DECLARE\n                            v_inner NUMBER := 3;\n                        BEGIN\n                            WHILE v_inner > 0 LOOP"
        ),
        "nested DECLARE/WHILE block inside ELSE should keep procedure-body depth, got: {formatted}"
    );
    assert!(
        formatted.contains("EXIT outer_loop WHEN v_inner = - 1;"),
        "labeled EXIT WHEN should stay on one line inside the nested loop, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "v_sql := q'[\n        SELECT empno,\n               ename,\n               sal,\n               CASE"
        ),
        "q-quoted dynamic SQL block should remain multiline with CASE layout, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "FORALL i IN 1..v_ids.COUNT SAVE EXCEPTIONS\n            UPDATE emp\n            SET sal = sal * 1.1\n            WHERE empno = v_ids (i);"
        ),
        "FORALL block should keep DML indentation and SAVE EXCEPTIONS on the loop header, got: {formatted}"
    );
    assert!(
        formatted.contains("FOR i IN 1..SQL%BULK_EXCEPTIONS.COUNT LOOP"),
        "SQL%BULK_EXCEPTIONS cursor attributes should not be split by spaces, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "SQL%BULK_EXCEPTIONS (i).ERROR_INDEX || ' CODE=' || SQL%BULK_EXCEPTIONS (i).ERROR_CODE"
        ),
        "BULK_EXCEPTIONS attribute access should stay attached to SQL%, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "<<main_loop>>\n        FOR i IN 1..5 LOOP\n            DECLARE\n                v_tmp NUMBER := i;\n            BEGIN"
        ),
        "deep nested main loop should preserve DECLARE/BEGIN structure, got: {formatted}"
    );
    assert!(
        formatted.contains("END torture_pkg;\n/"),
        "package body terminator should stay at the end of the formatted statement, got: {formatted}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(formatted, formatted_again);
}

#[test]
fn format_sql_fmt_pkg_extreme_script_keeps_package_body_and_following_blocks_separate() {
    let input = r#"--------------------------------------------------------------------------------
-- 0) 정리
--------------------------------------------------------------------------------
BEGIN
    EXECUTE IMMEDIATE 'DROP PACKAGE fmt_pkg_extreme';
EXCEPTION
    WHEN OTHERS THEN
        IF SQLCODE != -4043 THEN
            RAISE;
        END IF;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE fmtx_audit PURGE';
EXCEPTION
    WHEN OTHERS THEN
        IF SQLCODE != -942 THEN
            RAISE;
        END IF;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP TABLE fmtx_unit PURGE';
EXCEPTION
    WHEN OTHERS THEN
        IF SQLCODE != -942 THEN
            RAISE;
        END IF;
END;
/

BEGIN
    EXECUTE IMMEDIATE 'DROP SEQUENCE fmtx_audit_seq';
EXCEPTION
    WHEN OTHERS THEN
        IF SQLCODE != -2289 THEN
            RAISE;
        END IF;
END;
/
--------------------------------------------------------------------------------
-- 1) 테스트용 객체
--------------------------------------------------------------------------------
CREATE TABLE fmtx_unit (
    id         NUMBER PRIMARY KEY,
    parent_id  NUMBER,
    code       VARCHAR2(50)  NOT NULL,
    qty        NUMBER,
    price      NUMBER(12, 2),
    status     VARCHAR2(10),
    note       VARCHAR2(4000),
    created_at DATE DEFAULT SYSDATE,
    CONSTRAINT fk_fmtx_unit_parent
        FOREIGN KEY (parent_id)
        REFERENCES fmtx_unit (id)
);
/

CREATE TABLE fmtx_audit (
    audit_id    NUMBER PRIMARY KEY,
    phase       VARCHAR2(30),
    message     VARCHAR2(4000),
    created_at  TIMESTAMP DEFAULT SYSTIMESTAMP
);
/

CREATE SEQUENCE fmtx_audit_seq
    START WITH 1
    INCREMENT BY 1
    NOCACHE;
/

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (1, NULL, 'ROOT',      2, 100, 'NEW',  'root node');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (2, 1,    'ORD-A',     4,  15, 'OPEN', 'child a');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (3, 1,    'ORD-B',     1,  80, 'HOLD', 'child b');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (4, 2,    'ORD-A-01', 10,   5, 'DONE', 'leaf a-01');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (5, 2,    'ORD-A-02',  7,  12, 'NEW',  'leaf a-02');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (6, 3,    'ORD-B-01',  3,  25, 'OPEN', 'leaf b-01');

INSERT INTO fmtx_unit (id, parent_id, code, qty, price, status, note)
VALUES (7, 3,    'ORD-B-02',  8,   9, 'HOLD', 'leaf b-02');

COMMIT;
/
--------------------------------------------------------------------------------
-- 2) package spec
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE fmt_pkg_extreme AS
    TYPE t_num_aat IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    TYPE t_vc_aat  IS TABLE OF VARCHAR2(32767) INDEX BY PLS_INTEGER;

    PROCEDURE run_extreme(
        p_root_id IN NUMBER DEFAULT 1,
        p_text    OUT CLOB
    );

    PROCEDURE validate_and_process(
        p_root_id IN NUMBER,
        p_mode    IN VARCHAR2 DEFAULT 'NORMAL'
    );

    FUNCTION calc_score(
        p_qty    IN NUMBER,
        p_price  IN NUMBER,
        p_status IN VARCHAR2,
        p_depth  IN PLS_INTEGER DEFAULT 0
    ) RETURN NUMBER;

    FUNCTION render_snapshot(
        p_root_id IN NUMBER
    ) RETURN CLOB;
END fmt_pkg_extreme;
/
--------------------------------------------------------------------------------
-- 3) package body
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY fmt_pkg_extreme AS
    c_pkg_name   CONSTANT VARCHAR2(30) := 'FMT_PKG_EXTREME';
    c_max_depth  CONSTANT PLS_INTEGER  := 9;

    g_exec_count NUMBER       := 0;
    g_last_mode  VARCHAR2(30) := 'BOOT';

    SUBTYPE t_status IS VARCHAR2(10);

    ----------------------------------------------------------------------------
    -- audit log
    ----------------------------------------------------------------------------
    PROCEDURE audit(
        p_phase   IN VARCHAR2,
        p_message IN VARCHAR2
    ) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO fmtx_audit (
            audit_id,
            phase,
            message,
            created_at
        )
        VALUES (
            fmtx_audit_seq.NEXTVAL,
            SUBSTR(UPPER(p_phase), 1, 30),
            SUBSTR(p_message, 1, 4000),
            SYSTIMESTAMP
        );

        COMMIT;
    EXCEPTION
        WHEN OTHERS THEN
            ROLLBACK;
    END audit;

    ----------------------------------------------------------------------------
    -- score calculation
    ----------------------------------------------------------------------------
    FUNCTION calc_score(
        p_qty    IN NUMBER,
        p_price  IN NUMBER,
        p_status IN VARCHAR2,
        p_depth  IN PLS_INTEGER DEFAULT 0
    ) RETURN NUMBER IS
        l_score       NUMBER := 0;
        l_multiplier  NUMBER := 1;
        l_status      t_status := UPPER(TRIM(NVL(p_status, 'NEW')));

        FUNCTION base_score(
            p_qty_inner   IN NUMBER,
            p_price_inner IN NUMBER
        ) RETURN NUMBER IS
            l_base NUMBER := 0;
        BEGIN
            l_base :=
                  NVL(p_qty_inner, 0) * NVL(p_price_inner, 0)
                + CASE
                      WHEN NVL(p_qty_inner, 0) >= 10 THEN 25
                      WHEN NVL(p_qty_inner, 0) >= 5  THEN 10
                      ELSE 3
                  END;

            RETURN l_base;
        END base_score;

        FUNCTION tier_bonus(
            p_status_inner IN VARCHAR2
        ) RETURN NUMBER IS
        BEGIN
            RETURN CASE UPPER(TRIM(NVL(p_status_inner, 'NEW')))
                       WHEN 'DONE' THEN 40
                       WHEN 'OPEN' THEN 20
                       WHEN 'HOLD' THEN -5
                       ELSE 0
                   END;
        END tier_bonus;
    BEGIN
        l_score := base_score(p_qty, p_price) + tier_bonus(l_status);

        <<score_policy>>
        DECLARE
            l_depth_bonus NUMBER := 0;
        BEGIN
            IF p_depth <= 0 THEN
                l_depth_bonus := 0;
            ELSIF p_depth = 1 THEN
                l_depth_bonus := 2;
            ELSIF p_depth BETWEEN 2 AND 3 THEN
                l_depth_bonus := 7;
            ELSE
                l_depth_bonus := 15;
            END IF;

            CASE
                WHEN l_status IN ('NEW', 'OPEN') THEN
                    l_multiplier := 1.10;
                WHEN l_status = 'DONE' THEN
                    l_multiplier := 1.35;
                WHEN l_status = 'HOLD' THEN
                    l_multiplier := 0.80;
                ELSE
                    l_multiplier := 1;
            END CASE;

            l_score := ROUND((l_score + l_depth_bonus) * l_multiplier, 2);
        EXCEPTION
            WHEN VALUE_ERROR THEN
                l_score := -1;
        END score_policy;

        RETURN l_score;
    EXCEPTION
        WHEN OTHERS THEN
            RETURN -999999;
    END calc_score;

    ----------------------------------------------------------------------------
    -- current snapshot
    ----------------------------------------------------------------------------
    FUNCTION render_snapshot(
        p_root_id IN NUMBER
    ) RETURN CLOB IS
        l_out CLOB := TO_CLOB('');

        PROCEDURE push(
            p_line IN VARCHAR2
        ) IS
        BEGIN
            l_out := l_out || p_line || CHR(10);
        END push;
    BEGIN
        push('=== SNAPSHOT START ===');
        push('PACKAGE=' || c_pkg_name || ', EXEC_COUNT=' || g_exec_count || ', LAST_MODE=' || g_last_mode);

        FOR r IN (
            SELECT LEVEL AS lvl,
                   id,
                   parent_id,
                   LPAD(' ', (LEVEL - 1) * 2) || code AS tree_code,
                   qty,
                   price,
                   status
              FROM fmtx_unit
             START WITH id = p_root_id
           CONNECT BY PRIOR id = parent_id
             ORDER SIBLINGS BY id
        ) LOOP
            push(
                   '[' || r.lvl || '] '
                || 'ID=' || r.id
                || ', PARENT=' || NVL(TO_CHAR(r.parent_id), 'NULL')
                || ', CODE=' || r.tree_code
                || ', QTY=' || NVL(TO_CHAR(r.qty), 'NULL')
                || ', PRICE=' || NVL(TO_CHAR(r.price, 'FM9999990.00'), 'NULL')
                || ', STATUS=' || NVL(r.status, 'NULL')
                || ', BAND='
                || CASE
                       WHEN NVL(r.price, 0) >= 50 THEN 'HIGH'
                       WHEN NVL(r.price, 0) >= 10 THEN 'MID'
                       ELSE 'LOW'
                   END
            );
        END LOOP;

        push('=== SNAPSHOT END ===');
        RETURN l_out;
    EXCEPTION
        WHEN OTHERS THEN
            RETURN 'SNAPSHOT ERROR: ' || SQLERRM;
    END render_snapshot;

    ----------------------------------------------------------------------------
    -- main processor
    ----------------------------------------------------------------------------
    PROCEDURE validate_and_process(
        p_root_id IN NUMBER,
        p_mode    IN VARCHAR2 DEFAULT 'NORMAL'
    ) IS
        CURSOR c_units(cp_root_id NUMBER) IS
            SELECT id,
                   parent_id,
                   code,
                   qty,
                   price,
                   status,
                   note,
                   LEVEL AS lvl
              FROM fmtx_unit
             START WITH id = cp_root_id
           CONNECT BY PRIOR id = parent_id
             ORDER SIBLINGS BY id;

        TYPE t_units_tab IS TABLE OF c_units%ROWTYPE INDEX BY PLS_INTEGER;

        l_units      t_units_tab;
        l_ids        t_num_aat;
        l_marks      t_vc_aat;
        l_idx        PLS_INTEGER;
        l_mode       VARCHAR2(30) := UPPER(TRIM(NVL(p_mode, 'NORMAL')));
        l_total      NUMBER := 0;
        l_sql        VARCHAR2(32767);
        l_count      NUMBER := 0;

        e_bad_mode    EXCEPTION;
        e_deadlock    EXCEPTION;
        e_bulk_errors EXCEPTION;

        PRAGMA EXCEPTION_INIT(e_deadlock, -60);
        PRAGMA EXCEPTION_INIT(e_bulk_errors, -24381);

        PROCEDURE ensure_mode IS
        BEGIN
            IF l_mode NOT IN ('NORMAL', 'STRICT', 'AGGRESSIVE', 'DRYRUN') THEN
                RAISE e_bad_mode;
            END IF;
        END ensure_mode;

        FUNCTION decorate_note(
            p_old   IN VARCHAR2,
            p_score IN NUMBER,
            p_seq   IN PLS_INTEGER
        ) RETURN VARCHAR2 IS
        BEGIN
            RETURN SUBSTR(
                       NVL(p_old, '')
                    || CASE
                           WHEN p_old IS NULL THEN ''
                           ELSE CHR(10)
                       END
                    || q'~[fmt-begin
quotes: 'single', "double", q'[inner]'
purpose: formatter stress test
]~'
                    || 'seq=' || p_seq
                    || ', score=' || TO_CHAR(p_score, 'FM9999990.00')
                    || CHR(10)
                    || q'~[fmt-end]~',
                       1,
                       4000
                   );
        END decorate_note;

        PROCEDURE apply_one(
            p_row IN c_units%ROWTYPE,
            p_pos IN PLS_INTEGER
        ) IS
            l_score       NUMBER := 0;
            l_new_status  VARCHAR2(10);
        BEGIN
            SAVEPOINT sp_apply_one;

            IF l_mode = 'STRICT' AND p_row.qty IS NULL THEN
                RAISE_APPLICATION_ERROR(-20001, 'qty is required for id=' || p_row.id);
            END IF;

            l_score := calc_score(
                           p_qty    => p_row.qty,
                           p_price  => p_row.price,
                           p_status => p_row.status,
                           p_depth  => p_row.lvl
                       );

            <<inner_rules>>
            DECLARE
                l_counter PLS_INTEGER := 0;
                l_gate    VARCHAR2(10) := 'INIT';
            BEGIN
                LOOP
                    l_counter := l_counter + 1;

                    IF l_counter = 1 THEN
                        l_gate := 'FIRST';
                    ELSIF l_counter BETWEEN 2 AND 3 THEN
                        l_gate := 'MID';
                    ELSE
                        l_gate := 'STOP';
                    END IF;

                    EXIT WHEN l_gate = 'STOP';
                END LOOP;
            EXCEPTION
                WHEN OTHERS THEN
                    audit('INNER_RULES', 'inner_rules failed for id=' || p_row.id);
            END inner_rules;

            CASE
                WHEN l_score >= 150 THEN
                    l_new_status := 'DONE';
                WHEN l_score >= 80 THEN
                    l_new_status := 'OPEN';
                WHEN l_score >= 40 THEN
                    l_new_status := 'HOLD';
                ELSE
                    l_new_status := 'NEW';
            END CASE;

            l_sql := q'[
MERGE INTO fmtx_unit t
USING (
    SELECT :1 AS id,
           :2 AS status,
           :3 AS note
      FROM dual
) s
   ON (t.id = s.id)
 WHEN MATCHED THEN
      UPDATE
         SET t.status = s.status,
             t.note   = s.note,
             t.price  = CASE
                           WHEN t.price IS NULL THEN 0
                           ELSE t.price
                        END
]';

            EXECUTE IMMEDIATE l_sql
                USING p_row.id,
                      l_new_status,
                      decorate_note(p_row.note, l_score, p_pos);

            l_ids(p_pos)   := p_row.id;
            l_marks(p_pos) := l_new_status || ':' || TO_CHAR(l_score, 'FM9999990.00');
            l_total        := l_total + l_score;
        EXCEPTION
            WHEN e_deadlock THEN
                ROLLBACK TO sp_apply_one;
                audit('DEADLOCK', 'deadlock for id=' || p_row.id);
                RAISE;
            WHEN OTHERS THEN
                ROLLBACK TO sp_apply_one;
                audit(
                    'ROW_ERROR',
                    'apply_one failed for id=' || p_row.id || ': ' || SQLERRM
                );

                IF l_mode = 'STRICT' THEN
                    RAISE;
                END IF;
        END apply_one;
    BEGIN
        ensure_mode;

        g_exec_count := g_exec_count + 1;
        g_last_mode  := l_mode;

        OPEN c_units(p_root_id);
        FETCH c_units BULK COLLECT INTO l_units;
        CLOSE c_units;

        IF l_units.COUNT = 0 THEN
            audit('INFO', 'no rows for root_id=' || p_root_id);
            RETURN;
        END IF;

        l_idx := l_units.FIRST;

        <<scan_loop>>
        WHILE l_idx IS NOT NULL LOOP
            IF l_units(l_idx).lvl > c_max_depth THEN
                l_idx := l_units.NEXT(l_idx);
                CONTINUE scan_loop;
            END IF;

            BEGIN
                CASE
                    WHEN l_mode = 'DRYRUN' THEN
                        audit(
                            'DRYRUN',
                            'skip id=' || l_units(l_idx).id || ', code=' || l_units(l_idx).code
                        );

                    WHEN l_mode = 'AGGRESSIVE' THEN
                        apply_one(l_units(l_idx), l_idx);

                        IF l_units(l_idx).status = 'HOLD' THEN
                            apply_one(l_units(l_idx), l_idx);
                        END IF;

                    ELSE
                        apply_one(l_units(l_idx), l_idx);
                END CASE;
            EXCEPTION
                WHEN OTHERS THEN
                    audit(
                        'LOOP_ERROR',
                        'scan_loop item failed. idx=' || l_idx || ', id=' || l_units(l_idx).id
                    );

                    IF l_mode = 'STRICT' THEN
                        RAISE;
                    END IF;
            END;

            l_idx := l_units.NEXT(l_idx);
        END LOOP scan_loop;

        IF l_mode <> 'DRYRUN' AND l_ids.COUNT > 0 THEN
            BEGIN
                FORALL i IN INDICES OF l_ids SAVE EXCEPTIONS
                    UPDATE fmtx_unit
                       SET note =
                               SUBSTR(
                                   NVL(note, '')
                                || CASE
                                       WHEN note IS NULL THEN ''
                                       ELSE CHR(10)
                                   END
                                || '[batch-mark:' || l_marks(i) || ']',
                                   1,
                                   4000
                               )
                     WHERE id = l_ids(i);
            EXCEPTION
                WHEN e_bulk_errors THEN
                    FOR j IN 1 .. SQL%BULK_EXCEPTIONS.COUNT LOOP
                        audit(
                            'FORALL_ERR',
                            'index=' || SQL%BULK_EXCEPTIONS(j).ERROR_INDEX
                            || ', code=' || SQL%BULK_EXCEPTIONS(j).ERROR_CODE
                        );
                    END LOOP;
            END;
        END IF;

        BEGIN
            EXECUTE IMMEDIATE q'[
                SELECT COUNT(*)
                  FROM fmtx_unit
                 WHERE parent_id = :x
            ]'
                INTO l_count
                USING p_root_id;

            audit(
                'SUMMARY',
                'root_id=' || p_root_id
                || ', mode=' || l_mode
                || ', rows=' || l_units.COUNT
                || ', child_count=' || l_count
                || ', total=' || TO_CHAR(l_total, 'FM9999990.00')
            );
        EXCEPTION
            WHEN OTHERS THEN
                audit('SUMMARY_ERR', 'summary failed for root_id=' || p_root_id);
        END;
    EXCEPTION
        WHEN e_bad_mode THEN
            audit('BAD_MODE', 'unsupported mode=' || l_mode);
            RAISE_APPLICATION_ERROR(-20002, 'unsupported mode: ' || l_mode);
        WHEN OTHERS THEN
            audit(
                'FATAL',
                'validate_and_process failed for root_id=' || p_root_id || ': ' || SQLERRM
            );
            RAISE;
    END validate_and_process;

    ----------------------------------------------------------------------------
    -- orchestrator
    ----------------------------------------------------------------------------
    PROCEDURE run_extreme(
        p_root_id IN NUMBER DEFAULT 1,
        p_text    OUT CLOB
    ) IS
        l_modes     t_vc_aat;
        l_snapshot  CLOB;
        l_done_cnt  NUMBER;
        l_open_cnt  NUMBER;
        l_hold_cnt  NUMBER;

        PROCEDURE add_line(
            p_target IN OUT NOCOPY CLOB,
            p_line   IN VARCHAR2
        ) IS
        BEGIN
            p_target := p_target || p_line || CHR(10);
        END add_line;
    BEGIN
        p_text := TO_CLOB('');

        l_modes(1) := 'NORMAL';
        l_modes(2) := 'AGGRESSIVE';
        l_modes(3) := 'DRYRUN';

        FOR i IN 1 .. l_modes.COUNT LOOP
            BEGIN
                add_line(p_text, '=== MODE ' || l_modes(i) || ' START ===');

                validate_and_process(
                    p_root_id => p_root_id,
                    p_mode    => l_modes(i)
                );

                l_snapshot := render_snapshot(p_root_id);

                add_line(p_text, DBMS_LOB.SUBSTR(l_snapshot, 32767, 1));
                add_line(p_text, '=== MODE ' || l_modes(i) || ' END ===');
            EXCEPTION
                WHEN OTHERS THEN
                    add_line(
                        p_text,
                        'MODE ' || l_modes(i) || ' ERROR: ' || SQLERRM
                    );
            END;
        END LOOP;

        <<summary_block>>
        BEGIN
            SELECT SUM(CASE WHEN status = 'DONE' THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status = 'OPEN' THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status = 'HOLD' THEN 1 ELSE 0 END)
              INTO l_done_cnt,
                   l_open_cnt,
                   l_hold_cnt
              FROM fmtx_unit;

            add_line(
                p_text,
                'FINAL SUMMARY => DONE=' || NVL(l_done_cnt, 0)
                || ', OPEN=' || NVL(l_open_cnt, 0)
                || ', HOLD=' || NVL(l_hold_cnt, 0)
            );
        EXCEPTION
            WHEN OTHERS THEN
                add_line(p_text, 'FINAL SUMMARY ERROR');
        END summary_block;
    EXCEPTION
        WHEN OTHERS THEN
            p_text := 'RUN_EXTREME_ERROR: ' || SQLERRM;
    END run_extreme;

BEGIN
    g_last_mode :=
        CASE
            WHEN TO_CHAR(SYSDATE, 'DY', 'NLS_DATE_LANGUAGE=ENGLISH') IN ('SAT', 'SUN')
                THEN 'WEEKEND_BOOT'
            ELSE 'WEEKDAY_BOOT'
        END;

    audit('INIT', 'package initialized. mode=' || g_last_mode);
END fmt_pkg_extreme;
/
--------------------------------------------------------------------------------
-- 4) 실행 테스트
--------------------------------------------------------------------------------
SET SERVEROUTPUT ON;

DECLARE
    l_text CLOB;
BEGIN
    fmt_pkg_extreme.run_extreme(1, l_text);
    DBMS_OUTPUT.PUT_LINE(DBMS_LOB.SUBSTR(l_text, 32767, 1));
END;
/
--------------------------------------------------------------------------------
-- 5) 결과 확인
--------------------------------------------------------------------------------
SELECT id,
       parent_id,
       code,
       qty,
       price,
       status
  FROM fmtx_unit
 ORDER BY id;
/

SELECT audit_id,
       phase,
       message,
       created_at
  FROM fmtx_audit
 ORDER BY audit_id;
/"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(
            "PROCEDURE AUDIT (p_phase IN VARCHAR2, p_message IN VARCHAR2) IS\n        PRAGMA AUTONOMOUS_TRANSACTION;\n    BEGIN"
        ),
        "autonomous transaction procedure should keep declaration/body structure, got: {formatted}"
    );
    assert!(
        formatted.contains("TYPE t_units_tab IS TABLE OF c_units%ROWTYPE INDEX BY PLS_INTEGER;"),
        "%ROWTYPE attributes should stay attached inside package body declarations, got: {formatted}"
    );
    assert!(
        formatted.contains("FOR j IN 1..SQL%BULK_EXCEPTIONS.COUNT LOOP"),
        "SQL%BULK_EXCEPTIONS references should not be split by spaces, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "END fmt_pkg_extreme;\n/\n\n--------------------------------------------------------------------------------\n-- 4) 실행 테스트"
        ),
        "package body should close before the following execution block, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "DECLARE\n    l_text CLOB;\nBEGIN\n    fmt_pkg_extreme.run_extreme (1, l_text);"
        ),
        "following anonymous execution block should remain separate after package body formatting, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "END run_extreme;\nBEGIN\n    g_last_mode :="
        ),
        "package initializer BEGIN should recover to package-body top level after the last member END, got: {formatted}"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(formatted, formatted_again);
}

#[test]
fn format_sql_fmt_pkg_extreme_package_body_keeps_member_recovery_after_nested_exception_sections() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY fmt_pkg_extreme AS
    PROCEDURE validate_and_process (p_root_id IN NUMBER, p_mode IN VARCHAR2 DEFAULT 'NORMAL') IS
        PROCEDURE apply_one (p_pos IN PLS_INTEGER) IS
            l_score NUMBER := 0;
        BEGIN
            <<inner_rules>>
            DECLARE
                l_counter PLS_INTEGER := 0;
            BEGIN
                NULL;
            EXCEPTION
                WHEN OTHERS THEN
                    AUDIT ('INNER_RULES', 'inner_rules failed');
            END inner_rules;
        CASE
            WHEN l_score >= 150 THEN
                NULL;
            ELSE
                NULL;
        END CASE;
        EXCEPTION
            WHEN OTHERS THEN
                RAISE;
        END apply_one;
    BEGIN
        BEGIN
            EXECUTE IMMEDIATE q'[
                SELECT COUNT(*)
                  FROM fmtx_unit
                 WHERE parent_id = :x
            ]'
            INTO l_count USING p_root_id;
            AUDIT ('SUMMARY', 'root_id=' || p_root_id);
    EXCEPTION
        WHEN OTHERS THEN
            AUDIT ('SUMMARY_ERR', 'summary failed for root_id=' || p_root_id);
END;

EXCEPTION
    WHEN e_bad_mode THEN AUDIT ('BAD_MODE', 'unsupported mode=' || l_mode);

RAISE_APPLICATION_ERROR (- 20002, 'unsupported mode: ' || l_mode);

    WHEN OTHERS THEN AUDIT ('FATAL', 'validate_and_process failed for root_id=' || p_root_id || ': ' || SQLERRM);

RAISE;

END validate_and_process;

PROCEDURE run_extreme (p_root_id IN NUMBER DEFAULT 1, p_text OUT CLOB) IS l_modes t_vc_aat;

l_snapshot CLOB;

BEGIN
    NULL;
END run_extreme;
END fmt_pkg_extreme;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("END inner_rules;\n            CASE"),
        "statement after labeled inner block should stay in the same procedure body depth, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "EXCEPTION\n        WHEN e_bad_mode THEN\n            AUDIT ('BAD_MODE', 'unsupported mode=' || l_mode);\n            RAISE_APPLICATION_ERROR (- 20002, 'unsupported mode: ' || l_mode);"
        ),
        "package member exception handlers should expand inline THEN bodies into the exception block, got: {formatted}"
    );
    assert!(
        formatted.contains(
            "END validate_and_process;\n\n    PROCEDURE run_extreme (p_root_id IN NUMBER DEFAULT 1, p_text OUT CLOB) IS\n        l_modes t_vc_aat;\n        l_snapshot CLOB;"
        ),
        "formatter should recover package body member context before the next procedure declaration, got: {formatted}"
    );
}
#[test]
fn format_sql_declare_begin_pre_dedent() {
    let input = r#"DECLARE
v_old_sal NUMBER;
BEGIN
NULL;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = [
        "DECLARE",
        "    v_old_sal NUMBER;",
        "BEGIN",
        "    NULL;",
        "END;",
    ]
    .join("\n");

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_parser_depth_covers_loop_subquery_with_and_package_body() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY pkg_demo AS
PROCEDURE run_demo IS
BEGIN
FOR r IN (
SELECT id
FROM (
SELECT id FROM dual
)
) LOOP
NULL;
END LOOP;
END run_demo;
END pkg_demo;

WITH cte AS (
SELECT 1 AS n FROM dual
)
SELECT * FROM cte;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("PACKAGE BODY pkg_demo AS\n    PROCEDURE run_demo IS"),
        "Package body scope should increase depth, got: {}",
        formatted
    );
    assert!(
        formatted.contains("PROCEDURE run_demo IS\n    BEGIN"),
        "Procedure BEGIN should align with procedure declaration, got: {}",
        formatted
    );
    assert!(
        formatted.contains("        FOR r IN (\n            SELECT id"),
        "Subquery SELECT should increase depth, got: {}",
        formatted
    );
    assert!(
        formatted.contains("        ) LOOP\n            NULL;\n        END LOOP;"),
        "LOOP body should be indented one level deeper, got: {}",
        formatted
    );
    assert!(
        formatted
            .contains("WITH cte AS (\n    SELECT 1 AS n\n    FROM DUAL\n)\nSELECT *\nFROM cte;"),
        "WITH CTE block should increase depth and restore on main SELECT, got: {}",
        formatted
    );
}

#[test]
fn format_sql_formats_multi_cte_join_subquery_depth_consistently() {
    let input = "WITH emp_base AS (SELECT e.empno, e.ename, e.deptno, e.sal, e.hiredate FROM emp e WHERE e.hiredate >= DATE '2010-01-01'), dept_agg AS (SELECT eb.deptno, COUNT(*) AS emp_cnt, AVG(eb.sal) AS avg_sal FROM emp_base eb GROUP BY eb.deptno) SELECT d.deptno, d.dname, d.loc, c.emp_cnt, c.avg_sal, (SELECT MAX(eb2.sal) FROM emp_base eb2 WHERE eb2.deptno = c.deptno) AS max_sal_in_dept FROM dept d JOIN dept_agg c ON c.deptno = d.deptno WHERE d.loc = 'SEOUL' AND c.emp_cnt > 3 ORDER BY c.avg_sal DESC;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"WITH emp_base AS (
    SELECT
        e.empno,
        e.ename,
        e.deptno,
        e.sal,
        e.hiredate
    FROM emp e
    WHERE e.hiredate >= DATE '2010-01-01'
),
dept_agg AS (
    SELECT
        eb.deptno,
        COUNT(*) AS emp_cnt,
        AVG (eb.sal) AS avg_sal
    FROM emp_base eb
    GROUP BY eb.deptno
)
SELECT
    d.deptno,
    d.dname,
    d.loc,
    c.emp_cnt,
    c.avg_sal,
    (
        SELECT MAX (eb2.sal)
        FROM emp_base eb2
        WHERE eb2.deptno = c.deptno
    ) AS max_sal_in_dept
FROM dept d
JOIN dept_agg c
    ON c.deptno = d.deptno
WHERE d.loc = 'SEOUL'
    AND c.emp_cnt > 3
ORDER BY c.avg_sal DESC;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_formats_multi_cte_with_comments_and_scalar_subquery_exactly() {
    let input = "WITH e AS (SELECT empno, ename, job, mgr, hiredate, sal, comm, deptno FROM oqt_t_emp), d AS (SELECT deptno, dname, loc FROM oqt_t_dept), stats AS (SELECT deptno, COUNT(*) cnt, AVG(sal) avg_sal, SUM(NVL(comm, 0)) sum_comm FROM e GROUP BY deptno) SELECT d.deptno, d.dname, d.loc, s.cnt, ROUND(s.avg_sal, 2) AS avg_sal, s.sum_comm, -- scalar subquery (correlated)\n(SELECT MAX(e2.sal) FROM e e2 WHERE e2.deptno = d.deptno) AS max_sal_in_dept, -- case + analytic in select list via scalar subquery\nCASE WHEN s.cnt = 0 THEN 'EMPTY' WHEN s.avg_sal >= 2500 THEN 'HIGH' ELSE 'NORMAL' END AS dept_grade FROM d LEFT JOIN stats s ON s.deptno = d.deptno ORDER BY d.deptno;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"WITH e AS (
    SELECT
        empno,
        ename,
        job,
        mgr,
        hiredate,
        sal,
        comm,
        deptno
    FROM oqt_t_emp
),
d AS (
    SELECT
        deptno,
        dname,
        loc
    FROM oqt_t_dept
),
stats AS (
    SELECT
        deptno,
        COUNT(*) cnt,
        AVG (sal) avg_sal,
        SUM (NVL (comm, 0)) sum_comm
    FROM e
    GROUP BY deptno
)
SELECT
    d.deptno,
    d.dname,
    d.loc,
    s.cnt,
    ROUND (s.avg_sal, 2) AS avg_sal,
    s.sum_comm, -- scalar subquery (correlated)
    (
        SELECT MAX (e2.sal)
        FROM e e2
        WHERE e2.deptno = d.deptno
    ) AS max_sal_in_dept, -- case + analytic in select list via scalar subquery
    CASE
        WHEN s.cnt = 0 THEN 'EMPTY'
        WHEN s.avg_sal >= 2500 THEN 'HIGH'
        ELSE 'NORMAL'
    END AS dept_grade
FROM d
LEFT JOIN stats s
    ON s.deptno = d.deptno
ORDER BY d.deptno;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_cte_comment_layout_is_idempotent() {
    let input = r#"WITH e AS (
    SELECT
        empno,
        ename,
        job,
        mgr,
        hiredate,
        sal,
        comm,
        deptno
    FROM oqt_t_emp
),
d AS (
    SELECT
        deptno,
        dname,
        loc
    FROM oqt_t_dept
),
stats AS (
    SELECT
        deptno,
        COUNT(*) cnt,
        AVG (sal) avg_sal,
        SUM (NVL (comm, 0)) sum_comm
    FROM e
    GROUP BY deptno
)
SELECT
    d.deptno,
    d.dname,
    d.loc,
    s.cnt,
    ROUND (s.avg_sal, 2) AS avg_sal,
    s.sum_comm,
    -- scalar subquery (correlated)
    (
        SELECT MAX (e2.sal)
        FROM e e2
        WHERE e2.deptno = d.deptno
    ) AS max_sal_in_dept,
    -- case + analytic in select list via scalar subquery
    CASE
        WHEN s.cnt = 0 THEN 'EMPTY'
        WHEN s.avg_sal >= 2500 THEN 'HIGH'
        ELSE 'NORMAL'
    END AS dept_grade
FROM d
LEFT JOIN stats s
    ON s.deptno = d.deptno
ORDER BY d.deptno;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert_eq!(formatted, input);
}

#[test]
fn format_sql_from_subqueries_with_comma_aligns_as_expected() {
    let input =
        "SELECT * FROM (SELECT * FROM help) a, (SELECT * FROM help) b WHERE a.TOPIC = b.TOPIC;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"SELECT *
FROM (
        SELECT *
        FROM help
    ) a,
    (
        SELECT *
        FROM help
    ) b
WHERE a.TOPIC = b.TOPIC;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_filtered_cte_with_window_function_exact_layout() {
    let input = "filtered AS (SELECT * FROM enriched WHERE (sal > (SELECT AVG(sal) FROM oqt_t_emp WHERE deptno = enriched.deptno)) OR (job IN ('MANAGER', 'ANALYST') AND sal >= 2500)) SELECT f.deptno, f.dname, f.empno, f.ename, f.masked_name, f.job, f.sal, f.sal_band, -- window frame with last_value (needs careful frame)\nLAST_VALUE(f.sal) OVER (PARTITION BY f.deptno ORDER BY f.sal ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS max_sal_via_last_value FROM filtered f ORDER BY f.deptno, f.sal DESC, f.empno;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"filtered AS (
    SELECT *
    FROM enriched
    WHERE (sal > (
            SELECT AVG (sal)
            FROM oqt_t_emp
            WHERE deptno = enriched.deptno
        ))
        OR (job IN ('MANAGER', 'ANALYST')
            AND sal >= 2500)
)
SELECT f.deptno,
    f.dname,
    f.empno,
    f.ename,
    f.masked_name,
    f.job,
    f.sal,
    f.sal_band, -- window frame with last_value (needs careful frame)
    LAST_VALUE (f.sal) OVER (
        PARTITION BY f.deptno
        ORDER BY f.sal
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ) AS max_sal_via_last_value
FROM filtered f
ORDER BY f.deptno,
    f.sal DESC,
    f.empno;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_window_functions_and_listagg_exact_layout() {
    let input = "WITH base AS (SELECT e.empno, e.ename, e.deptno, e.sal, e.hiredate FROM oqt_t_emp e) SELECT b.*, RANK() OVER (PARTITION BY deptno ORDER BY sal DESC) AS rnk, DENSE_RANK() OVER (PARTITION BY deptno ORDER BY sal DESC) AS drnk, ROW_NUMBER() OVER (PARTITION BY deptno ORDER BY hiredate, empno) AS rn, SUM(sal) OVER (PARTITION BY deptno) AS sum_sal_dept, AVG(sal) OVER (PARTITION BY deptno) AS avg_sal_dept, PERCENT_RANK() OVER (PARTITION BY deptno ORDER BY sal) AS pct_rank, CUME_DIST() OVER (PARTITION BY deptno ORDER BY sal) AS CUME_DIST, -- running total with frame\nSUM(sal) OVER (PARTITION BY deptno ORDER BY hiredate ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running_sal, -- windowed listagg\nLISTAGG(ename, ',') WITHIN GROUP (ORDER BY ename) OVER (PARTITION BY deptno) AS names_in_dept FROM base b ORDER BY deptno, rnk, empno;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"WITH base AS (
    SELECT
        e.empno,
        e.ename,
        e.deptno,
        e.sal,
        e.hiredate
    FROM oqt_t_emp e
)
SELECT
    b.*,
    RANK () OVER (
        PARTITION BY deptno
        ORDER BY sal DESC
    ) AS rnk,
    DENSE_RANK () OVER (
        PARTITION BY deptno
        ORDER BY sal DESC
    ) AS drnk,
    ROW_NUMBER () OVER (
        PARTITION BY deptno
        ORDER BY hiredate,
        empno
    ) AS rn,
    SUM (sal) OVER (
        PARTITION BY deptno
    ) AS sum_sal_dept,
    AVG (sal) OVER (
        PARTITION BY deptno
    ) AS avg_sal_dept,
    PERCENT_RANK () OVER (
        PARTITION BY deptno
        ORDER BY sal
    ) AS pct_rank,
    CUME_DIST () OVER (
        PARTITION BY deptno
        ORDER BY sal
    ) AS CUME_DIST, -- running total with frame
    SUM (sal) OVER (
        PARTITION BY deptno
        ORDER BY hiredate
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ) AS running_sal, -- windowed listagg
    LISTAGG (ename, ',') WITHIN
    GROUP (ORDER BY ename) OVER (
        PARTITION BY deptno
    ) AS names_in_dept
FROM base b
ORDER BY deptno,
    rnk,
    empno;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_resets_comma_break_suppression_after_unbalanced_paren_semicolon() {
    let input = "SELECT func(a, b;\nSELECT c, d FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT c,\n    d\nFROM DUAL;"),
        "Second statement should recover to normal SELECT-list wrapping, got: {}",
        formatted
    );
    assert!(
        !formatted.contains("SELECT\n    c,\n    d\nFROM DUAL;"),
        "Invalid statement should not force stale recovery layout on next statement, got: {}",
        formatted
    );
}

#[test]
fn format_sql_slash_separator_does_not_force_next_statement_comma_wrapping() {
    let input = "SELECT func(a, b;\n/\nSELECT c, d FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("SELECT c,\n    d\nFROM DUAL;"),
        "Second statement should keep normal comma wrapping after slash separator, got: {}",
        formatted
    );
    assert!(
        !formatted.contains("SELECT\n    c,\n    d\nFROM DUAL;"),
        "Slash separator should not trigger forced SELECT-line break recovery pattern, got: {}",
        formatted
    );
}

#[test]
fn format_sql_comment_parenthesis_does_not_affect_comma_newline() {
    let input = "SELECT a /* (comment) */, b FROM dual";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("/* (comment) */,\n    b"),
        "Parenthesis inside comments must not keep comma on one line, got: {}",
        formatted
    );
}

#[test]
fn format_sql_cursor_loop_with_case_and_exit_when_keeps_loop_body_layout() {
    let input = r#"FOR r IN (
SELECT id,
grp,
n,
CASE
WHEN n < 0 THEN 'NEG'
WHEN n = 0 THEN 'ZERO'
WHEN MOD (n, 2) = 0 THEN 'EVEN'
ELSE 'ODD'
END AS kind
FROM oqt_t_test
WHERE grp = p_grp
ORDER BY id FETCH FIRST 6 ROWS ONLY
) LOOP v_depth := v_depth + 1;
log_msg ('RUN', 'loop', v_depth, 'id=' || r.id || ' n=' || r.n || ' kind=' || r.kind);
EXIT
WHEN v_depth > 10;
END LOOP;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains(") LOOP\n    v_depth := v_depth + 1;"),
        "Loop body should start on a new line after LOOP, got: {}",
        formatted
    );
    assert!(
        formatted.contains("EXIT WHEN v_depth > 10;"),
        "EXIT WHEN should stay on one line, got: {}",
        formatted
    );
}

#[test]
fn format_sql_exit_when_with_label_stays_on_one_line() {
    let input = r#"FOR i IN 1..10 LOOP
EXIT outer_loop
WHEN i > 5;
END LOOP;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("EXIT outer_loop WHEN i > 5;"),
        "Labeled EXIT WHEN should stay on one line, got: {}",
        formatted
    );
}

#[test]
fn format_sql_package_loop_select_case_end_alignment_regression() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY oqt_mega_pkg AS
    PROCEDURE run_torture (p_grp IN NUMBER, p_n IN NUMBER, p_txt IN VARCHAR2) IS
        PROCEDURE jumpy (p IN NUMBER) IS
            -- 커서 루프 + EXIT WHEN + CASE(expression)
            FOR r IN (
                SELECT id,
                    grp,
                    n,
                    CASE
                        WHEN n < 0 THEN 'NEG'
                        WHEN n = 0 THEN 'ZERO'
                        WHEN MOD (n, 2) = 0 THEN 'EVEN'
                        ELSE 'ODD'
                     END AS kind
                FROM oqt_t_test
                WHERE grp = p_grp
                ORDER BY id FETCH FIRST 6 ROWS ONLY
            ) LOOP
                v_depth := v_depth + 1;
                log_msg ('RUN', 'loop', v_depth, 'id=' || r.id || ' n=' || r.n || ' kind=' || r.kind);
                EXIT WHEN v_depth > 10;
            END LOOP;
        END run_torture;
    END oqt_mega_pkg;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CASE\n                        WHEN n < 0 THEN 'NEG'"),
        "CASE/WHEN alignment in SELECT list is broken, got: {}",
        formatted
    );
    assert!(
        formatted.contains("ELSE 'ODD'\n                    END AS kind"),
        "CASE END should align with CASE in SELECT list, got: {}",
        formatted
    );
}

#[test]
fn format_sql_open_cursor_nested_case_expression_alignment_regression() {
    let input = r#"CREATE OR REPLACE PACKAGE BODY oqt_mega_pkg AS
    PROCEDURE open_rc (p_grp IN NUMBER, p_rc OUT SYS_REFCURSOR) IS
    BEGIN
        OPEN p_rc FOR
            SELECT t.id,
                t.grp,
                t.n,
                CASE
                WHEN t.grp = 0 THEN
                CASE
                    WHEN t.n > 10 THEN 'G0_BIG'
                    ELSE 'G0_SMALL'
                END
                WHEN t.grp IN (1, 2) THEN 'G12'
                ELSE 'GOTHER'
            END AS bucket,
                SUBSTR (t.txt, 1, 200) AS txt
            FROM oqt_t_test t
            WHERE t.grp = p_grp
            ORDER BY t.id;
    END open_rc;
END oqt_mega_pkg;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CASE\n                    WHEN t.grp = 0 THEN"),
        "Outer CASE/WHEN alignment in OPEN FOR SELECT is broken, got: {}",
        formatted
    );
    assert!(
        formatted.contains("WHEN t.grp IN (1, 2) THEN 'G12'\n                    ELSE 'GOTHER'"),
        "CASE branches should align in OPEN FOR SELECT, got: {}",
        formatted
    );
    assert!(
        formatted.contains("ELSE 'GOTHER'\n                END AS bucket,"),
        "CASE END should align with CASE in OPEN FOR SELECT, got: {}",
        formatted
    );
}

#[test]
fn format_sql_case_end_parenthesis_breaks_line_in_plsql_expression() {
    let input = r#"BEGIN
FOR i IN 1..3 LOOP
v_sum := v_sum + (
CASE
WHEN MOD (i, 2) = 0 THEN
i * 10
ELSE
i
END);
END LOOP;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains(
            "v_sum := v_sum + (\n            CASE\n                WHEN MOD (i, 2) = 0 THEN"
        ),
        "CASE block inside parenthesized expression should be one depth deeper, got: {}",
        formatted
    );
    assert!(
        formatted.contains("END\n        );"),
        "CASE END and closing parenthesis should be split across lines and the pure close should return to the paren owner depth, got: {}",
        formatted
    );
    assert!(
        !formatted.contains("END);"),
        "END); should not stay on one line in this pattern, got: {}",
        formatted
    );
}

#[test]
fn format_sql_paren_case_end_with_comment_does_not_leak_depth_to_next_line() {
    let input = r#"BEGIN
v_num := (
CASE
WHEN 1 = 1 THEN
1
ELSE
0
END -- close case
);
v_next := 2;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains("END -- close case\n    );\n    v_next := 2;"),
        "parenthesized CASE depth should be closed before the next statement, got: {formatted}"
    );
}

#[test]
fn format_sql_paren_case_start_with_inline_comment_keeps_case_indented() {
    let input = r#"BEGIN
v_num := ( -- keep this comment
CASE
WHEN 1 = 1 THEN
1
ELSE
0
END
);
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains("v_num := ( -- keep this comment\n        CASE\n            WHEN 1 = 1 THEN"),
        "CASE block after parenthesis+comment should stay indented as expression depth, got: {formatted}"
    );
    assert!(
        formatted.contains("END\n    );"),
        "closing parenthesis should realign with the parenthesized expression owner depth, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_case_start_with_inline_comment_keeps_case_indented() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT ( -- inline comment
CASE
WHEN score > 10 THEN 'HIGH'
ELSE 'LOW'
END
) AS bucket
FROM dual;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    assert!(
        formatted.contains("( -- inline comment\n                CASE\n                    WHEN score > 10 THEN 'HIGH'"),
        "OPEN FOR nested CASE after an opening paren comment should indent one level deeper than the paren line, got: {formatted}"
    );
    assert!(
        formatted.contains("END\n            ) AS bucket"),
        "CASE END and close paren should preserve stable OPEN FOR expression indentation, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_nested_paren_case_with_inline_comment_keeps_closing_depth() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT (
CASE
WHEN score > 10 THEN (
CASE -- nested expression
WHEN score > 20 THEN 'HIGH+'
ELSE 'HIGH'
END
)
ELSE 'LOW'
END
) AS bucket
FROM dual;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.len().saturating_sub(line.trim_start().len());

    let outer_open_idx = lines
        .iter()
        .position(|line| line.trim() == "(")
        .expect("should contain outer expression wrapper");
    let outer_case_idx = lines
        .iter()
        .enumerate()
        .skip(outer_open_idx + 1)
        .find(|(_, line)| line.trim_start() == "CASE")
        .map(|(idx, _)| idx)
        .expect("should contain outer CASE");
    let inner_open_idx = lines
        .iter()
        .enumerate()
        .skip(outer_case_idx + 1)
        .find(|(_, line)| line.trim() == "(")
        .map(|(idx, _)| idx)
        .expect("should contain inner expression wrapper");
    let inner_case_idx = lines
        .iter()
        .enumerate()
        .skip(inner_open_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("CASE -- nested expression"))
        .map(|(idx, _)| idx)
        .expect("should contain inner CASE");
    let inner_end_idx = lines
        .iter()
        .enumerate()
        .skip(inner_case_idx + 1)
        .find(|(_, line)| line.trim() == "END")
        .map(|(idx, _)| idx)
        .expect("should contain inner END");
    let inner_close_idx = lines
        .iter()
        .enumerate()
        .skip(inner_end_idx + 1)
        .find(|(_, line)| line.trim() == ")")
        .map(|(idx, _)| idx)
        .expect("should contain inner close paren");
    let else_idx = lines
        .iter()
        .enumerate()
        .skip(inner_close_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with("ELSE 'LOW'"))
        .map(|(idx, _)| idx)
        .expect("should contain ELSE 'LOW'");
    let outer_end_idx = lines
        .iter()
        .enumerate()
        .skip(else_idx + 1)
        .find(|(_, line)| line.trim() == "END")
        .map(|(idx, _)| idx)
        .expect("should contain outer END");
    let outer_close_idx = lines
        .iter()
        .enumerate()
        .skip(outer_end_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with(") AS bucket"))
        .map(|(idx, _)| idx)
        .expect("should contain outer close paren");

    assert_eq!(
        indent(lines[outer_case_idx]),
        indent(lines[outer_open_idx]).saturating_add(4),
        "outer CASE body should be one level deeper than its wrapper line, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[inner_case_idx]),
        indent(lines[inner_open_idx]).saturating_add(4),
        "inner CASE body should be one level deeper than its wrapper line, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[inner_close_idx]),
        indent(lines[else_idx]),
        "inner close-paren should return to the surrounding branch depth, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[outer_close_idx]),
        indent(lines[outer_open_idx]),
        "outer close-paren should return to the OPEN FOR expression wrapper depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_trigger_if_elsif_alignment_matches_expected() {
    let input = r#"CREATE OR REPLACE NONEDITIONABLE TRIGGER "SYSTEM"."OQT_TRG_CHILD_BIU"
BEFORE
    INSERT OR UPDATE ON oqt_t_child
FOR EACH ROW
BEGIN
    IF INSERTING THEN
        :NEW.updated_at := NULL;
        IF :NEW.note IS
        NULL THEN
            :NEW.note := 'auto-note:' || :NEW.sku;
        END IF;
    ELSIF UPDATING THEN
        :NEW.updated_at := SYSDATE;
    END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"CREATE OR REPLACE NONEDITIONABLE TRIGGER "SYSTEM"."OQT_TRG_CHILD_BIU"
    BEFORE INSERT OR UPDATE
    ON oqt_t_child
    FOR EACH ROW
BEGIN
    IF INSERTING THEN
        :NEW.updated_at := NULL;
        IF :NEW.note IS
        NULL THEN
            :NEW.note := 'auto-note:' || :NEW.sku;
        END IF;
    ELSIF UPDATING THEN
        :NEW.updated_at := SYSDATE;
    END IF;
END;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn format_sql_parenthesized_if_condition_continuation_uses_single_extra_indent() {
    let input = r#"BEGIN
    IF (i = 2
                AND b = 2) THEN
        RAISE_APPLICATION_ERROR (- 20002, 'forced nested error i=2 b=2');
    END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"BEGIN
    IF (i = 2
        AND b = 2) THEN
        RAISE_APPLICATION_ERROR (- 20002, 'forced nested error i=2 b=2');
    END IF;
END;"#;

    assert_eq!(formatted, expected);
}

#[test]
fn resolve_serveroutput_enable_size_uses_default_after_unlimited_session_state() {
    assert_eq!(
        SqlEditorWidget::resolve_serveroutput_enable_size(None, 0, 1_000_000),
        1_000_000
    );
    assert_eq!(
        SqlEditorWidget::resolve_serveroutput_enable_size(Some(2_000), 0, 1_000_000),
        2_000
    );
    assert_eq!(
        SqlEditorWidget::resolve_serveroutput_enable_size(None, 50_000, 1_000_000),
        50_000
    );
}

#[test]
fn finalize_execution_state_clears_running_and_cancel_flags() {
    let query_running = Arc::new(Mutex::new(true));
    let cancel_flag = Arc::new(Mutex::new(true));

    SqlEditorWidget::finalize_execution_state(&query_running, &cancel_flag);

    assert!(!load_mutex_bool(&query_running));
    assert!(!load_mutex_bool(&cancel_flag));
}

// ── q-quote after identifier: tokenizer regression ──

#[test]
fn tokenize_sql_identifier_ending_q_not_treated_as_q_quote() {
    // `seq` is one identifier; the following `'text'` is a regular string.
    let tokens = SqlEditorWidget::tokenize_sql("SELECT seq'text' FROM dual");
    let has_word_seq = tokens
        .iter()
        .any(|t| matches!(t, SqlToken::Word(w) if w == "seq"));
    assert!(
        has_word_seq,
        "Identifier 'seq' should be a single Word token, got: {:?}",
        tokens
    );
    let has_q_quote_string = tokens
        .iter()
        .any(|t| matches!(t, SqlToken::String(s) if s.starts_with("q'")));
    assert!(
        !has_q_quote_string,
        "Should NOT produce a q-quote String token when q is part of an identifier, got: {:?}",
        tokens
    );
}

#[test]
fn tokenize_sql_identifier_ending_nq_not_treated_as_nq_quote() {
    // `unq` is one identifier; the following `'val'` is a regular string.
    let tokens = SqlEditorWidget::tokenize_sql("SELECT unq'val' FROM dual");
    let has_word_unq = tokens
        .iter()
        .any(|t| matches!(t, SqlToken::Word(w) if w == "unq"));
    assert!(
        has_word_unq,
        "Identifier 'unq' should be a single Word token, got: {:?}",
        tokens
    );
}

#[test]
fn tokenize_sql_standalone_q_quote_still_works() {
    // Standalone q'[...]' must still be recognized.
    let tokens = SqlEditorWidget::tokenize_sql("SELECT q'[hello]' FROM dual");
    let has_q_string = tokens
        .iter()
        .any(|t| matches!(t, SqlToken::String(s) if s.starts_with("q'")));
    assert!(
        has_q_string,
        "Standalone q-quote should produce a String token, got: {:?}",
        tokens
    );
}

#[test]
fn tokenize_sql_standalone_nq_quote_still_works() {
    let tokens = SqlEditorWidget::tokenize_sql("SELECT nq'[test]' FROM dual");
    let has_nq_string = tokens
        .iter()
        .any(|t| matches!(t, SqlToken::String(s) if s.starts_with("nq'")));
    assert!(
        has_nq_string,
        "Standalone nq-quote should produce a String token, got: {:?}",
        tokens
    );
}

// ── OPEN CURSOR FOR: nested structures & parentheses ─────────────────

#[test]
fn format_sql_open_cursor_for_nested_subquery_in_where() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a, b
FROM t1
WHERE a IN (
SELECT x FROM t2 WHERE y > 0
)
ORDER BY a;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // OPEN p_rc FOR should be at indent 1
    let open_line = lines.iter().find(|l| l.contains("OPEN p_rc FOR")).unwrap();
    let open_indent = open_line.len() - open_line.trim_start().len();
    assert_eq!(
        open_indent, 4,
        "OPEN should be at indent 1 (4 spaces), got: {formatted}"
    );

    // SELECT should be at indent 2
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();
    assert_eq!(
        select_indent, 8,
        "SELECT should be at indent 2, got: {formatted}"
    );

    // WHERE should be at same level as SELECT
    let where_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("WHERE a IN"))
        .unwrap();
    let where_indent = where_line.len() - where_line.trim_start().len();
    assert_eq!(
        where_indent, 8,
        "WHERE should be at indent 2, got: {formatted}"
    );

    // Subquery SELECT inside IN() should be indented further
    let sub_select = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT x"))
        .unwrap();
    let sub_indent = sub_select.len() - sub_select.trim_start().len();
    assert!(
        sub_indent > where_indent,
        "subquery SELECT should be deeper than WHERE, got: {formatted}"
    );

    // ORDER BY should be back at the outer SELECT level
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_nested_parens_in_select_list() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT
NVL(a, (SELECT MAX(x) FROM t2)),
b,
c
FROM t1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Verify the NVL line stays coherent and the FROM returns to correct depth
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT in OPEN FOR, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_deeply_nested_parens() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT DECODE(a, 1, (CASE WHEN b > 0 THEN (c + d) ELSE 0 END), 0) AS val
FROM t1
WHERE EXISTS (SELECT 1 FROM t2 WHERE t2.id = t1.id);
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Verify FROM returns to correct level after deeply nested parens
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT DECODE"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    assert_eq!(
        from_indent, select_indent,
        "FROM should stay at SELECT level after nested parens, got: {formatted}"
    );

    // WHERE should also align
    let where_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("WHERE EXISTS"))
        .unwrap();
    let where_indent = where_line.len() - where_line.trim_start().len();
    assert_eq!(
        where_indent, select_indent,
        "WHERE should stay at SELECT level, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_multiple_in_same_block() {
    let input = r#"BEGIN
OPEN rc1 FOR
SELECT a FROM t1 WHERE a > 0;

OPEN rc2 FOR
SELECT b FROM t2 WHERE b < 10
ORDER BY b;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    // Both OPEN lines should have the same indent
    let open_lines: Vec<&str> = formatted
        .lines()
        .filter(|l| l.trim_start().starts_with("OPEN "))
        .collect();
    assert_eq!(
        open_lines.len(),
        2,
        "should have 2 OPEN lines, got: {formatted}"
    );

    let indent1 = open_lines[0].len() - open_lines[0].trim_start().len();
    let indent2 = open_lines[1].len() - open_lines[1].trim_start().len();
    assert_eq!(
        indent1, indent2,
        "both OPEN should have same indent, got: {formatted}"
    );

    // Second OPEN's SELECT should also be properly indented
    let select_lines: Vec<&str> = formatted
        .lines()
        .filter(|l| l.trim_start().starts_with("SELECT "))
        .collect();
    assert_eq!(
        select_lines.len(),
        2,
        "should have 2 SELECT lines, got: {formatted}"
    );

    let sel_indent1 = select_lines[0].len() - select_lines[0].trim_start().len();
    let sel_indent2 = select_lines[1].len() - select_lines[1].trim_start().len();
    assert_eq!(
        sel_indent1, sel_indent2,
        "both SELECTs should have same indent, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_with_nested_case_and_subquery() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT id,
CASE
WHEN status IN (SELECT code FROM ref_table) THEN 'VALID'
WHEN status = 'X' THEN 'EXPIRED'
ELSE 'UNKNOWN'
END AS status_label
FROM main_table
WHERE active = 1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // CASE should be indented deeper than SELECT columns
    let case_line = lines.iter().find(|l| l.trim_start() == "CASE").unwrap();
    let case_indent = case_line.len() - case_line.trim_start().len();

    // END AS should align with CASE
    let end_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("END AS"))
        .unwrap();
    let end_indent = end_line.len() - end_line.trim_start().len();

    assert_eq!(
        case_indent, end_indent,
        "CASE and END should align, got: {formatted}"
    );

    // FROM should be back at SELECT level
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM main_table"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT in OPEN FOR, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_double_nested_parens() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT ((a + b) * (c - d)) AS calc,
e
FROM t1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // FROM should return to SELECT level
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();

    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT after double-nested parens, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_nested_open_in_if_block() {
    let input = r#"BEGIN
IF p_mode = 1 THEN
OPEN p_rc FOR
SELECT a, b FROM t1
WHERE a > 0;
ELSE
OPEN p_rc FOR
SELECT c, d FROM t2
WHERE c < 10;
END IF;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // OPEN inside IF should be at indent 2 (BEGIN + IF)
    let open_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("OPEN p_rc FOR"))
        .copied()
        .collect();
    assert_eq!(
        open_lines.len(),
        2,
        "should have 2 OPEN lines, got: {formatted}"
    );

    let open_indent = open_lines[0].len() - open_lines[0].trim_start().len();
    assert_eq!(
        open_indent, 8,
        "OPEN in IF should be at indent 2 (8 spaces), got: {formatted}"
    );

    // SELECT inside nested OPEN should be at indent 3
    let select_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("SELECT "))
        .copied()
        .collect();
    for sel in &select_lines {
        let sel_indent = sel.len() - sel.trim_start().len();
        assert_eq!(
            sel_indent, 12,
            "SELECT in IF>OPEN should be at indent 3 (12 spaces), got: {formatted}"
        );
    }
}

#[test]
fn format_sql_open_cursor_for_with_union_all() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a, b FROM t1
UNION ALL
SELECT c, d FROM t2
ORDER BY 1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Both SELECTs should be at the same indent
    let select_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("SELECT "))
        .copied()
        .collect();
    assert_eq!(
        select_lines.len(),
        2,
        "should have 2 SELECT lines, got: {formatted}"
    );

    let indent1 = select_lines[0].len() - select_lines[0].trim_start().len();
    let indent2 = select_lines[1].len() - select_lines[1].trim_start().len();
    assert_eq!(
        indent1, indent2,
        "both SELECTs in UNION should have same indent, got: {formatted}"
    );

    // UNION ALL should be at the same level as SELECT
    let union_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("UNION ALL"))
        .unwrap();
    let union_indent = union_line.len() - union_line.trim_start().len();
    assert_eq!(
        union_indent, indent1,
        "UNION ALL should align with SELECT in OPEN FOR, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_paren_subquery_in_from() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a.id, b.val
FROM (
SELECT id, status FROM t1 WHERE active = 1
) a
JOIN t2 b ON b.id = a.id
WHERE b.val > 0;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // The outer FROM should be at SELECT level
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a.id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM (") || l.trim_start().starts_with("FROM("))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT, got: {formatted}"
    );

    // Subquery SELECT should be deeper
    let sub_select = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT id"))
        .unwrap();
    let sub_indent = sub_select.len() - sub_select.trim_start().len();
    assert!(
        sub_indent > from_indent,
        "subquery SELECT should be deeper than FROM, got: {formatted}"
    );

    // JOIN should be at outer FROM level
    let join_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("JOIN "))
        .unwrap();
    let join_indent = join_line.len() - join_line.trim_start().len();
    assert_eq!(
        join_indent, from_indent,
        "JOIN should align with FROM, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_semicolon_resets_state() {
    // After OPEN FOR ... ;  the next statement should be back to normal indent
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a FROM t1;
v_count := 0;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // v_count assignment should be at indent 1 (BEGIN level), not at OPEN FOR level
    let assign_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("v_count"))
        .unwrap();
    let assign_indent = assign_line.len() - assign_line.trim_start().len();
    assert_eq!(
        assign_indent, 4,
        "statement after OPEN FOR should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_nested_case_with_nested_parens() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT
CASE
WHEN a = 1 THEN (SELECT MAX(x) FROM t2)
WHEN a = 2 THEN (
SELECT MIN(y) FROM t3
)
ELSE 0
END AS val,
b
FROM t1;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // END AS should align with CASE
    let case_line = lines.iter().find(|l| l.trim_start() == "CASE").unwrap();
    let case_indent = case_line.len() - case_line.trim_start().len();

    let end_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("END AS val"))
        .unwrap();
    let end_indent = end_line.len() - end_line.trim_start().len();
    assert_eq!(
        case_indent, end_indent,
        "CASE and END AS should align even with subquery parens inside, got: {formatted}"
    );

    // FROM should return to SELECT level
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT after nested CASE+subquery, got: {formatted}"
    );
}

// ── OPEN CURSOR FOR: extended edge cases ─────────────────────────────

#[test]
fn format_sql_open_cursor_for_with_cte_clause_depth_regression() {
    // CTE WITH inside OPEN FOR: the outer SELECT/FROM should align,
    // even though the CTE body has its own SELECT at deeper depth.
    let input = r#"BEGIN
OPEN p_rc FOR
WITH cte AS (
SELECT id, name FROM t_src WHERE active = 1
)
SELECT cte.id,
cte.name,
t2.val
FROM cte
JOIN t2 ON t2.id = cte.id
WHERE t2.val > 0
ORDER BY cte.id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Find the main (outer) SELECT that follows the CTE closing paren
    let cte_close_idx = lines.iter().position(|l| l.trim_start() == ")").unwrap();
    let outer_select_line = lines[cte_close_idx + 1..]
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT"))
        .unwrap();
    let outer_select_indent = outer_select_line.len() - outer_select_line.trim_start().len();

    // FROM cte should align with outer SELECT
    let from_cte_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM cte"))
        .unwrap();
    let from_cte_indent = from_cte_line.len() - from_cte_line.trim_start().len();

    assert_eq!(
        from_cte_indent, outer_select_indent,
        "FROM cte should align with outer SELECT, got: {formatted}"
    );

    // ORDER BY should also align
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, outer_select_indent,
        "ORDER BY should align with outer SELECT, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_dynamic_sql_using_does_not_break() {
    // OPEN cursor FOR dynamic_sql USING param — no SELECT follows
    let input = r#"BEGIN
OPEN p_cursor FOR v_sql USING p_dept;
v_count := 0;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // v_count should be at indent 1 (not affected by OPEN FOR)
    let assign_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("v_count"))
        .unwrap();
    let assign_indent = assign_line.len() - assign_line.trim_start().len();
    assert_eq!(
        assign_indent, 4,
        "statement after dynamic OPEN FOR should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_with_multiple_cte() {
    let input = r#"BEGIN
OPEN p_rc FOR
WITH cte1 AS (
SELECT id FROM t1
),
cte2 AS (
SELECT id FROM t2
)
SELECT cte1.id, cte2.id
FROM cte1
JOIN cte2 ON cte2.id = cte1.id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Find main SELECT (after the second CTE closing paren)
    let last_cte_close_idx = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == ")")
        .map(|(idx, _)| idx)
        .next_back()
        .unwrap();
    let main_select = lines
        .iter()
        .skip(last_cte_close_idx.saturating_add(1))
        .find(|l| {
            let trimmed = l.trim_start();
            trimmed == "SELECT" || trimmed.starts_with("SELECT cte1")
        })
        .unwrap();
    let main_select_indent = main_select.len() - main_select.trim_start().len();

    // FROM should align with main SELECT
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM cte1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();

    assert_eq!(
        from_indent, main_select_indent,
        "FROM should align with main SELECT after multiple CTEs, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_subquery_in_from_and_where() {
    // Both FROM and WHERE have subqueries
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a.id, a.val
FROM (SELECT id, val FROM t1 WHERE val > 0) a
WHERE a.id IN (SELECT ref_id FROM t2)
ORDER BY a.id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a.id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    // ORDER BY should align with outer SELECT after nested subqueries in both FROM and WHERE
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT after subqueries in FROM+WHERE, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_in_loop_block() {
    let input = r#"BEGIN
FOR rec IN 1..3 LOOP
OPEN p_rc FOR
SELECT a, b FROM t1
WHERE a = rec;
CLOSE p_rc;
END LOOP;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // OPEN inside FOR LOOP should be at indent 2
    let open_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("OPEN p_rc FOR"))
        .unwrap();
    let open_indent = open_line.len() - open_line.trim_start().len();
    assert_eq!(
        open_indent, 8,
        "OPEN in LOOP should be at indent 2, got: {formatted}"
    );

    // SELECT should be at indent 3
    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();
    assert_eq!(
        select_indent, 12,
        "SELECT in LOOP>OPEN should be at indent 3, got: {formatted}"
    );

    // CLOSE should be back at indent 2
    let close_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("CLOSE"))
        .unwrap();
    let close_indent = close_line.len() - close_line.trim_start().len();
    assert_eq!(
        close_indent, 8,
        "CLOSE after OPEN FOR should be at indent 2, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_correlated_subquery() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a.id,
a.val,
(SELECT COUNT(*) FROM t2 WHERE t2.aid = a.id) AS cnt
FROM t1 a
WHERE a.val > (SELECT AVG(val) FROM t1)
ORDER BY a.id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a.id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    // FROM should align with SELECT
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT after correlated subqueries, got: {formatted}"
    );

    // WHERE should align with SELECT
    let where_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("WHERE a.val"))
        .unwrap();
    let where_indent = where_line.len() - where_line.trim_start().len();
    assert_eq!(
        where_indent, select_indent,
        "WHERE should align with SELECT, got: {formatted}"
    );

    // ORDER BY should align with SELECT
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT after correlated subqueries, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_nested_exists_subqueries() {
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT id, name
FROM t1
WHERE EXISTS (
SELECT 1 FROM t2
WHERE t2.id = t1.id
AND EXISTS (
SELECT 1 FROM t3
WHERE t3.id = t2.id
)
)
ORDER BY id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    // ORDER BY should come back to outer SELECT level
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT after deeply nested EXISTS, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_with_cte_and_nested_subquery_in_main() {
    // CTE + nested subquery in WHERE of main query
    let input = r#"BEGIN
OPEN p_rc FOR
WITH base AS (
SELECT id, grp FROM t1 WHERE active = 1
)
SELECT b.id, b.grp
FROM base b
WHERE b.grp IN (SELECT grp FROM t2 WHERE priority > 0)
ORDER BY b.id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Outer SELECT
    let cte_close_idx = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.trim() == ")")
        .map(|(idx, _)| idx)
        .unwrap();
    let outer_select = lines
        .iter()
        .skip(cte_close_idx.saturating_add(1))
        .find(|l| {
            let trimmed = l.trim_start();
            trimmed == "SELECT" || trimmed.starts_with("SELECT b.id")
        })
        .unwrap();
    let outer_select_indent = outer_select.len() - outer_select.trim_start().len();

    // ORDER BY must align with the outer SELECT
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, outer_select_indent,
        "ORDER BY should align with outer SELECT after CTE+subquery, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_select_into_before_open_does_not_leak() {
    // A normal SELECT INTO before OPEN FOR should not interfere
    let input = r#"BEGIN
SELECT COUNT(*) INTO v_cnt FROM t0;
OPEN p_rc FOR
SELECT a, b FROM t1
ORDER BY a;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // SELECT a, b should be at indent 2 (OPEN FOR level)
    let sel_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a"))
        .unwrap();
    let sel_indent = sel_line.len() - sel_line.trim_start().len();
    assert_eq!(
        sel_indent, 8,
        "SELECT inside OPEN FOR should be at indent 2, got: {formatted}"
    );

    // ORDER BY should align with it
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, sel_indent,
        "ORDER BY should align with SELECT in OPEN FOR, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_all_clause_keywords_aligned() {
    // Comprehensive: all major clause keywords should align
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a, b, c
FROM t1
WHERE a > 0
GROUP BY b
HAVING COUNT(*) > 1
ORDER BY c;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    for keyword in &["FROM t1", "WHERE a", "GROUP BY", "HAVING COUNT", "ORDER BY"] {
        let kw_line = lines
            .iter()
            .find(|l| l.trim_start().starts_with(keyword))
            .unwrap_or_else(|| panic!("expected line starting with {keyword}, got: {formatted}"));
        let kw_indent = kw_line.len() - kw_line.trim_start().len();
        assert_eq!(
            kw_indent, select_indent,
            "{keyword} should align with SELECT, got: {formatted}"
        );
    }
}

#[test]
fn format_sql_open_cursor_for_multiline_subquery_in_select_list() {
    // Subquery in SELECT list that spans multiple lines
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT id,
(
SELECT MAX(val)
FROM t2
WHERE t2.id = t1.id
) AS max_val,
name
FROM t1
ORDER BY id;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    // FROM t1 and ORDER BY should be back at SELECT level
    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT after multiline subquery in SELECT list, got: {formatted}"
    );

    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_second_open_after_subquery_first() {
    // First OPEN has subquery, second OPEN should be independent
    let input = r#"BEGIN
OPEN rc1 FOR
SELECT a FROM t1
WHERE a IN (SELECT x FROM t2);

OPEN rc2 FOR
SELECT b FROM t3
ORDER BY b;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // Find second OPEN's SELECT
    let select_lines: Vec<(usize, &&str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("SELECT b"))
        .collect();
    assert!(
        !select_lines.is_empty(),
        "should have SELECT b line, got: {formatted}"
    );
    let second_select_indent = select_lines[0].1.len() - select_lines[0].1.trim_start().len();

    // ORDER BY in second OPEN should align with second SELECT
    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, second_select_indent,
        "ORDER BY in second OPEN should align with its SELECT, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_inline_subquery_on_same_line() {
    // Subquery entirely on one line shouldn't break indent
    let input = r#"BEGIN
OPEN p_rc FOR
SELECT a, (SELECT MAX(id) FROM t2) AS mx
FROM t1
ORDER BY a;
END;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT a"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT when subquery is inline, got: {formatted}"
    );

    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT, got: {formatted}"
    );
}

#[test]
fn format_sql_open_cursor_for_package_body_procedure() {
    // OPEN FOR inside a package body procedure
    let input = r#"CREATE OR REPLACE PACKAGE BODY my_pkg AS
PROCEDURE get_data(p_rc OUT SYS_REFCURSOR) IS
BEGIN
OPEN p_rc FOR
SELECT id,
name,
(SELECT COUNT(*) FROM t2 WHERE t2.pid = t1.id) AS child_cnt
FROM t1
WHERE active = 1
ORDER BY id;
END get_data;
END my_pkg;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let select_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SELECT id"))
        .unwrap();
    let select_indent = select_line.len() - select_line.trim_start().len();

    let from_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("FROM t1"))
        .unwrap();
    let from_indent = from_line.len() - from_line.trim_start().len();
    assert_eq!(
        from_indent, select_indent,
        "FROM should align with SELECT in package body OPEN FOR, got: {formatted}"
    );

    let order_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("ORDER BY"))
        .unwrap();
    let order_indent = order_line.len() - order_line.trim_start().len();
    assert_eq!(
        order_indent, select_indent,
        "ORDER BY should align with SELECT in package body OPEN FOR, got: {formatted}"
    );
}

// ── Multiline string + trailing code indent ──────────────────────────

#[test]
fn format_sql_multiline_string_preserves_content_with_trailing_code() {
    // The string content must not be altered by re-indentation.
    // After the closing quote, || b || 'c' should stay on the same line.
    let input = "BEGIN\n    a := 'b\n              b'     || b || 'c';\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    // The multiline string 'b\n              b' must be preserved exactly.
    assert!(
        formatted.contains("'b\n              b'"),
        "multiline string content must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_next_statement_indent() {
    // After a multiline string assignment, the next statement must
    // return to the correct PL/SQL block indent level.
    let input = "BEGIN\n    a := 'hello\nworld';\n    b := 1;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    let b_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("b :="))
        .unwrap();
    let b_indent = b_line.len() - b_line.trim_start().len();
    assert_eq!(
        b_indent, 4,
        "statement after multiline string should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_concat_then_next_statement() {
    // Multiline string with concatenation, followed by a new statement.
    let input = "BEGIN\n    v_msg := 'line1\nline2' || ' extra';\n    v_next := 0;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // String content preserved
    assert!(
        formatted.contains("'line1\nline2'"),
        "multiline string must be preserved, got: {formatted}"
    );

    // Next statement at correct indent
    let next_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("v_next"))
        .unwrap();
    let next_indent = next_line.len() - next_line.trim_start().len();
    assert_eq!(
        next_indent, 4,
        "next statement should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_with_large_indent_inside() {
    // String has very deep indentation inside — must not be altered.
    let input = "BEGIN\n    v := 'start\n                        deep inside\n    back';\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("'start\n                        deep inside\n    back'"),
        "multiline string deep indent must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_closing_quote_alone_on_line() {
    // Closing quote on its own line.
    let input = "BEGIN\n    v := 'hello\nworld\n';\n    b := 2;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // The string content must remain intact
    assert!(
        formatted.contains("'hello\nworld\n'"),
        "multiline string with trailing newline must be preserved, got: {formatted}"
    );

    // b := 2 at correct indent
    let b_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("b :="))
        .unwrap();
    let b_indent = b_line.len() - b_line.trim_start().len();
    assert_eq!(
        b_indent, 4,
        "statement after multiline string should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_in_select() {
    // Multiline string in a SELECT statement.
    let input = "SELECT 'hello\nworld' AS msg,\n    1 AS num\nFROM dual;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("'hello\nworld'"),
        "multiline string in SELECT must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_in_plsql_if_block() {
    // Multiline string inside nested IF block.
    let input = "BEGIN\n    IF cond THEN\n        v := 'a\n            b\n            c';\n        w := 1;\n    END IF;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // String content preserved
    assert!(
        formatted.contains("'a\n            b\n            c'"),
        "multiline string inside IF must be preserved, got: {formatted}"
    );

    // w := 1 at correct indent (inside IF, so indent 2)
    let w_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("w :="))
        .unwrap();
    let w_indent = w_line.len() - w_line.trim_start().len();
    assert_eq!(
        w_indent, 8,
        "statement after multiline string in IF should be at indent 2, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_followed_by_concat_on_next_line() {
    // Closing quote on its own line, then concat on a new line.
    let input = "BEGIN\n    v := 'part1\npart2'\n        || 'part3';\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    // String content preserved
    assert!(
        formatted.contains("'part1\npart2'"),
        "multiline string must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_multiple_multiline_strings_in_sequence() {
    // Two multiline strings in the same block.
    let input = "BEGIN\n    a := 'x\ny';\n    b := 'p\nq';\n    c := 1;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    assert!(
        formatted.contains("'x\ny'"),
        "first multiline string must be preserved, got: {formatted}"
    );
    assert!(
        formatted.contains("'p\nq'"),
        "second multiline string must be preserved, got: {formatted}"
    );

    let c_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("c :="))
        .unwrap();
    let c_indent = c_line.len() - c_line.trim_start().len();
    assert_eq!(
        c_indent, 4,
        "statement after two multiline strings should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_with_escaped_quotes() {
    // String with escaped quotes inside — must not be confused.
    let input = "BEGIN\n    v := 'it''s\na test';\n    w := 1;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("'it''s\na test'"),
        "multiline string with escaped quotes must be preserved, got: {formatted}"
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let w_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("w :="))
        .unwrap();
    let w_indent = w_line.len() - w_line.trim_start().len();
    assert_eq!(
        w_indent, 4,
        "statement after multiline string with escapes should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_q_quote_preserves_content() {
    // Q-quoted multiline string must be preserved.
    let input = "BEGIN\n    v := q'[hello\nworld]';\n    w := 1;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("q'[hello\nworld]'"),
        "multiline q-quote string must be preserved, got: {formatted}"
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let w_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("w :="))
        .unwrap();
    let w_indent = w_line.len() - w_line.trim_start().len();
    assert_eq!(
        w_indent, 4,
        "statement after q-quote multiline should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_deeply_indented_trailing_code() {
    // The user's exact scenario: multiline string with deep indent,
    // followed by concatenation on the same line as closing quote.
    let input = "BEGIN\n    a := 'b\n              b'     || b || 'c';\n    d := 1;\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();

    // The string 'b\n              b' must be exactly preserved
    assert!(
        formatted.contains("'b\n              b'"),
        "multiline string with deep indent must be preserved exactly, got: {formatted}"
    );

    // d := 1 must be at indent 1
    let d_line = lines
        .iter()
        .find(|l| l.trim_start().starts_with("d :="))
        .unwrap();
    let d_indent = d_line.len() - d_line.trim_start().len();
    assert_eq!(
        d_indent, 4,
        "statement after deeply-indented multiline string should be at indent 1, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_in_insert_values() {
    // Multiline string in INSERT VALUES clause
    let input = "INSERT INTO t1 (col1, col2)\nVALUES ('hello\nworld', 1);";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("'hello\nworld'"),
        "multiline string in INSERT VALUES must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_multiline_string_as_procedure_argument() {
    // Multiline string passed as procedure argument
    let input = "BEGIN\n    my_proc('arg1\narg2',\n        p_other => 1);\nEND;";

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("'arg1\narg2'"),
        "multiline string in procedure call must be preserved, got: {formatted}"
    );
}

#[test]
fn format_sql_oracle_final_boss_idempotent() {
    let input = load_test_file("oracle_format_final_boss.sql");
    assert!(
        !input.is_empty(),
        "Test file oracle_format_final_boss.sql should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);

    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for oracle_format_final_boss.sql"
    );
}

#[test]
fn full_auto_formatting_open_with_select_comment_does_not_gain_blank_line() {
    let input = load_test_file("test_open_with.sql");
    assert!(
        !input.is_empty(),
        "Test file test_open_with.sql should not be empty"
    );

    let formatted = SqlEditorWidget::format_for_auto_formatting(&input, false);

    assert!(
        formatted.contains(
            "            d.dname,\n            -- [D] 스칼라 서브쿼리 시작"
        ),
        "Full auto-formatting should keep the SELECT-list comment attached without an inserted blank line, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains(
            "            d.dname,\n\n            -- [D] 스칼라 서브쿼리 시작"
        ),
        "Full auto-formatting inserted an unexpected blank line before the SELECT-list comment, got:\n{}",
        formatted
    );
}

#[test]
fn split_format_items_does_not_treat_division_slash_as_terminator() {
    // Division operator `/` on its own line inside parenthesized expression
    // must NOT be treated as a SQL*Plus slash terminator.
    // `/` on its own line inside parentheses must not be a SQL*Plus slash terminator
    let cases: Vec<(&str, &str)> = vec![
        (
            "SELECT\n    (\n        (1 + 2)\n        /\n        NULLIF(x, 0)\n    ) AS result\nFROM dual",
            "nested parens with / on own line",
        ),
        (
            "SELECT (a\n/\nb) FROM dual",
            "simple paren with / on own line",
        ),
    ];

    for (input, label) in &cases {
        let items = crate::db::QueryExecutor::split_format_items(input);
        let slash_count = items
            .iter()
            .filter(|i| matches!(i, crate::db::FormatItem::Slash))
            .count();
        assert_eq!(
            slash_count, 0,
            "[{}] Division `/` inside parens should not be a slash terminator; items: {:?}",
            label, items
        );
    }
}

#[test]
fn split_format_items_does_not_treat_cte_alias_r_as_run_command() {
    // CTE alias `r` must NOT be treated as a RUN script command.
    let input =
        "WITH\n    a AS (SELECT 1 FROM dual),\n    r AS (SELECT 2 FROM dual)\nSELECT * FROM r";
    let items = crate::db::QueryExecutor::split_format_items(input);
    let tool_count = items
        .iter()
        .filter(|i| matches!(i, crate::db::FormatItem::ToolCommand(_)))
        .count();
    assert_eq!(
        tool_count, 0,
        "CTE alias `r` should not become a ToolCommand; items: {:?}",
        items
    );
}

#[test]
fn format_sql_oracle_ultimate_boss_idempotent() {
    let input = load_test_file("oracle_format_ultimate_boss.sql");
    assert!(
        !input.is_empty(),
        "Test file oracle_format_ultimate_boss.sql should not be empty"
    );
    let formatted = SqlEditorWidget::format_sql_basic(&input);
    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for oracle_format_ultimate_boss.sql"
    );
}

#[test]
fn format_sql_cache_and_no_cache_match_for_oracle_ultimate_boss() {
    let input = load_test_file("oracle_format_ultimate_boss.sql");
    assert!(
        !input.is_empty(),
        "Test file oracle_format_ultimate_boss.sql should not be empty"
    );

    let cached = SqlEditorWidget::format_sql_basic(&input);
    let uncached = SqlEditorWidget::format_sql_basic_no_cache(&input);

    assert_eq!(
        cached, uncached,
        "cache and no-cache formatting should match for oracle_format_ultimate_boss.sql"
    );
}

#[test]
fn format_sql_keeps_test_format_pivot_reference_layout_exactly() {
    let expected = load_test_file("test_format_pivot.sql")
        .trim_end()
        .to_string();
    assert!(
        !expected.is_empty(),
        "Test file test_format_pivot.sql should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic(&expected);
    assert_eq!(
        formatted, expected,
        "Formatting must preserve the reference pivot layout exactly"
    );

    let formatted_again = SqlEditorWidget::format_sql_basic(&formatted);
    assert_eq!(
        formatted, formatted_again,
        "Formatting should be idempotent for test_format_pivot.sql"
    );
}

#[test]
fn format_sql_keeps_multiline_start_with_clause_as_sql() {
    let input =
        "SELECT LEVEL\nFROM emp e\nSTART WITH\n    e.mgr IS NULL\nCONNECT BY PRIOR e.empno = e.mgr\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("START WITH e.mgr IS NULL")
            || formatted.contains("START WITH\n    e.mgr IS NULL"),
        "START WITH clause should remain SQL, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("@WITH"),
        "hierarchical START WITH must not become a script command, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_match_recognize_define_clause_as_sql() {
    let input = "SELECT *\nFROM ticks\nMATCH_RECOGNIZE (\n  PARTITION BY symbol\n  ORDER BY ts\n  PATTERN (STRT DOWN+)\n  DEFINE\n    DOWN AS price < PREV(price)\n)\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let formatted_items = QueryExecutor::split_script_items(&formatted);
    let tool_define_count = formatted_items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ScriptItem::ToolCommand(crate::db::ToolCommand::Define { .. })
            )
        })
        .count();

    assert_eq!(
        tool_define_count, 0,
        "MATCH_RECOGNIZE DEFINE clause must not be converted to SQL*Plus DEFINE command, got:\n{formatted}"
    );
    assert!(
        formatted.contains("DEFINE\n")
            || formatted.contains("DEFINE DOWN AS")
            || formatted.contains("DEFINE\n    DOWN AS"),
        "MATCH_RECOGNIZE DEFINE clause text should remain in SQL statement, got:\n{formatted}"
    );
}

#[test]
fn format_sql_keeps_split_for_update_lines_on_same_clause_depth() {
    let input = "SELECT e.empno,\n       e.sal\nFROM emp e\nFOR\nUPDATE OF e.sal NOWAIT\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FROM emp e"))
        .unwrap_or(0);
    let for_idx = lines
        .iter()
        .position(|line| line.trim_start() == "FOR")
        .unwrap_or(0);
    let update_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("UPDATE OF e.sal NOWAIT"))
        .unwrap_or(0);

    assert_eq!(
        indent(lines[for_idx]),
        indent(lines[from_idx]),
        "FOR in split SELECT FOR UPDATE should align with SELECT clause depth, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[update_idx]),
        indent(lines[from_idx]),
        "UPDATE in split SELECT FOR UPDATE should align with SELECT clause depth, got:\n{formatted}"
    );
}

#[test]
fn format_sql_keeps_desc_sort_modifier_attached_after_with_function_cte_query() {
    let input = "WITH\n    FUNCTION fmt_mask (p_txt IN VARCHAR2) RETURN VARCHAR2 IS\n    BEGIN\n        RETURN p_txt;\n    END fmt_mask,\n    base_emp AS (\n        SELECT\n            e.empno,\n            e.ename,\n            ROW_NUMBER () OVER (\n                PARTITION BY e.deptno\n                ORDER BY e.sal DESC, e.empno\n            ) AS rn\n        FROM emp e\n    )\nSELECT\n    b.empno,\n    b.ename\nFROM base_emp b\nORDER BY\n    b.empno,\n    CASE\n        WHEN b.rn = 1 THEN 1\n        ELSE 2\n    END,\n    b.ename DESC,\n    b.empno\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("b.ename DESC,\n    b.empno;"),
        "ORDER BY DESC fragment should stay attached to the preceding sort key, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("\n\nDESC,"),
        "DESC sort modifier must not split into a standalone statement, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_indents_first_select_item_under_with_function_cte_select_header() {
    let input = "WITH\n    FUNCTION fmt_mask (p_txt IN VARCHAR2) RETURN VARCHAR2 IS\n    BEGIN\n        RETURN p_txt;\n    END fmt_mask,\n    base_emp AS (\n        SELECT\n            e.empno,\n            e.ename\n        FROM emp e\n    )\nSELECT base_emp.empno\nFROM base_emp\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let select_idx = lines
        .iter()
        .position(|line| line.trim_start() == "SELECT" && line.starts_with("        "))
        .unwrap_or(0);
    let item_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("e.empno"))
        .unwrap_or(0);

    assert!(
        indent(lines[item_idx]) > indent(lines[select_idx]),
        "first select-list item inside WITH FUNCTION CTE SELECT should indent deeper than SELECT header, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_with_function_cte_scalar_subquery_uses_same_base_depth_rules() {
    let input = r#"WITH
    FUNCTION fmt_mask (p_txt IN VARCHAR2) RETURN VARCHAR2 IS
    BEGIN
        RETURN REGEXP_REPLACE (NVL (p_txt, 'NULL'), '([[:alnum:]])', '*');
    END fmt_mask,
    PROCEDURE noop (p_msg IN VARCHAR2) IS
    BEGIN
        NULL;
    END noop,
    base_emp AS (
        SELECT
            e.empno,
            e.ename,
            (
                SELECT MAX (x.sal)
                FROM emp x
                WHERE x.deptno = e.deptno
            ) AS max_sal
        FROM emp e
    )
SELECT
    b.empno
FROM base_emp b
;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let with_idx = lines
        .iter()
        .position(|line| line.trim_start() == "WITH")
        .unwrap_or(0);
    let cte_idx = lines
        .iter()
        .position(|line| line.trim_start() == "base_emp AS (")
        .unwrap_or(0);
    let cte_select_idx = lines
        .iter()
        .position(|line| line.trim_start() == "SELECT")
        .unwrap_or(0);
    let scalar_open_idx = lines
        .iter()
        .position(|line| line.trim_start() == "(")
        .unwrap_or(0);
    let scalar_select_idx = lines
        .iter()
        .enumerate()
        .skip(scalar_open_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start().starts_with("SELECT MAX"))
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    assert_eq!(
        indent(lines[cte_idx]),
        indent(lines[with_idx]),
        "WITH FUNCTION CTE header should stay aligned with the WITH owner depth, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[cte_select_idx]),
        indent(lines[cte_idx]).saturating_add(4),
        "CTE body SELECT inside WITH FUNCTION should stay exactly one level deeper than the CTE header, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[scalar_select_idx]),
        indent(lines[scalar_open_idx]).saturating_add(4),
        "scalar subquery inside WITH FUNCTION CTE should follow the same base-depth rule as other child queries, got:\n{formatted}"
    );
}

#[test]
fn format_sql_indents_insert_first_branches_under_when() {
    let input = "INSERT FIRST\n    WHEN deptno = 10 THEN\n        INTO emp_bucket_a (empno, deptno)\n        VALUES (empno, deptno)\n    WHEN deptno = 20 THEN\n        INTO emp_bucket_b (empno, deptno)\n        VALUES (empno, deptno)\n    ELSE\n        INTO emp_bucket_c (empno, deptno)\n        VALUES (empno, deptno)\nSELECT empno, deptno\nFROM emp\n;";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let when_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHEN deptno = 10 THEN"))
        .unwrap_or(0);
    let into_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("INTO emp_bucket_a"))
        .unwrap_or(0);
    let values_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("VALUES (empno, deptno)"))
        .unwrap_or(0);

    assert!(
        indent(lines[into_idx]) > indent(lines[when_idx]),
        "INSERT FIRST INTO branch should indent deeper than WHEN, got:\n{}",
        formatted
    );
    assert!(
        indent(lines[values_idx]) > indent(lines[when_idx]),
        "INSERT FIRST VALUES branch should indent deeper than WHEN, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_plsql_case_assignment_and_end_loop_depth() {
    let input = "DECLARE\n    v_text VARCHAR2 (100);\nBEGIN\n    v_text :=\n           CASE\n               WHEN 1 = 1 THEN\n                   'Y'\n               ELSE\n                   'N'\n           END;\n    FOR r IN (SELECT 1 AS n FROM dual) LOOP\n        v_text := v_text || r.n;\n    END LOOP;\nEND;\n/";
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let assign_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("v_text :="))
        .unwrap_or(0);
    let case_idx = lines
        .iter()
        .position(|line| line.trim_start() == "CASE")
        .unwrap_or(0);
    let loop_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FOR r IN"))
        .unwrap_or(0);
    let end_loop_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END LOOP;")
        .unwrap_or(0);

    assert!(
        indent(lines[case_idx]) >= indent(lines[assign_idx]),
        "CASE assigned after := should stay aligned with or deeper than the assignment line, got:\n{}",
        formatted
    );
    assert!(
        indent(lines[end_loop_idx]) == indent(lines[loop_idx]),
        "END LOOP should align with its FOR LOOP header, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_final_end_at_depth_zero_after_case_in_for_loop_header() {
    let input = r#"DECLARE
    v_x NUMBER := 1;
    v_y VARCHAR2(100);
BEGIN
    v_y := CASE
        WHEN v_x = 1 THEN
            CASE v_x
                WHEN 1 THEN 'ONE'
                WHEN 2 THEN 'TWO'
                ELSE CASE WHEN v_x > 2 THEN 'BIG' ELSE 'SMALL' END
            END
        WHEN v_x = 2 THEN
            'TWO'
        ELSE
            CASE
                WHEN v_x IS NULL THEN 'NULL'
                ELSE 'OTHER'
            END
    END;

    FOR i IN 1..CASE WHEN v_x = 1 THEN 5 ELSE 10 END LOOP
        DBMS_OUTPUT.PUT_LINE(
            CASE i WHEN 1 THEN 'first' WHEN 2 THEN 'second' ELSE 'other' END
        );
    END LOOP;

    UPDATE my_table
    SET status = CASE
        WHEN id IN (SELECT CASE WHEN active = 'Y' THEN id ELSE NULL END FROM sub_t) THEN 'ACTIVE'
        ELSE 'INACTIVE'
    END,
    priority = CASE category
        WHEN 'A' THEN 1
        WHEN 'B' THEN 2
        ELSE CASE WHEN amount > 1000 THEN 3 ELSE 4 END
    END
    WHERE dept_id = v_x;

    COMMIT;
END;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let loop_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FOR i IN 1.."))
        .unwrap_or(0);
    let end_loop_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END LOOP;")
        .unwrap_or(0);
    let commit_idx = lines
        .iter()
        .position(|line| line.trim_start() == "COMMIT;")
        .unwrap_or(0);
    let end_idx = lines
        .iter()
        .rposition(|line| line.trim_start() == "END;")
        .unwrap_or(0);

    assert_eq!(
        indent(lines[end_loop_idx]),
        indent(lines[loop_idx]),
        "END LOOP should align with FOR header after CASE range expression, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[commit_idx]),
        4,
        "COMMIT should stay one level deeper than the outer DECLARE/BEGIN block, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[end_idx]),
        0,
        "final END should return to depth 0 after mixed CASE/LOOP/UPDATE formatting, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_view_case_scalar_subquery_uses_parent_line_base_depth() {
    let input = r#"CREATE OR REPLACE VIEW v_emp_formatter_boss AS
SELECT
    MAX (
        CASE
            WHEN e.sal = (
                SELECT
                    MAX (x.sal)
                FROM
                    emp x
                WHERE
                    x.deptno = d.deptno
            ) THEN e.ename
            ELSE NULL
        END
    ) AS top_name
FROM dept d
LEFT JOIN emp e
    ON e.deptno = d.deptno;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();
    let when_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHEN e.sal = ("))
        .unwrap_or(0);
    let select_idx = lines
        .iter()
        .enumerate()
        .skip(when_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start().starts_with("SELECT MAX (x.sal)"))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FROM emp x"))
        .unwrap_or(0);
    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE x.deptno = d.deptno"))
        .unwrap_or(0);
    let close_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with(") THEN e.ename"))
        .unwrap_or(0);

    assert!(
        indent(lines[select_idx]) > indent(lines[when_idx]),
        "scalar subquery SELECT should indent deeper than its CASE WHEN owner line, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[from_idx]),
        indent(lines[select_idx]),
        "scalar subquery FROM should reuse the SELECT base depth, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[where_idx]),
        indent(lines[select_idx]),
        "scalar subquery WHERE should reuse the SELECT base depth, got:\n{formatted}"
    );
    assert_eq!(
        indent(lines[close_idx]),
        indent(lines[when_idx]),
        "closing parenthesis should realign with the CASE WHEN owner line, got:\n{formatted}"
    );
}

#[test]
fn format_sql_nested_operator_scalar_subquery_keeps_owner_and_child_query_depths_stable() {
    let input = r#"SELECT
    (
        SELECT COUNT(*)
        FROM (
                SELECT 1
                FROM employees e5
                WHERE e5.manager_id = e.employee_id
                        AND EXISTS (
                            SELECT 1
                            FROM employees e6
                            WHERE e6.manager_id = e5.employee_id
                                    AND e6.salary > (
                            SELECT PERCENTILE_CONT (0.75) WITHIN GROUP (ORDER BY salary)
                            FROM employees e7
                            WHERE e7.department_id = e5.department_id
                        )
                        )
            )
    ) AS deep_report_count
FROM a;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);
    let expected = r#"SELECT
    (
        SELECT COUNT(*)
        FROM (
                SELECT 1
                FROM employees e5
                WHERE e5.manager_id = e.employee_id
                    AND EXISTS (
                        SELECT 1
                        FROM employees e6
                        WHERE e6.manager_id = e5.employee_id
                            AND e6.salary > (
                                SELECT PERCENTILE_CONT (0.75) WITHIN GROUP (ORDER BY salary)
                                FROM employees e7
                                WHERE e7.department_id = e5.department_id
                            )
                    )
            )
    ) AS deep_report_count
FROM a;"#;

    assert_eq!(
        formatted, expected,
        "nested operator-owned scalar subqueries should follow the same owner/child ladder without wrapper double-counting"
    );
    assert_eq!(
        SqlEditorWidget::format_sql_basic(&formatted),
        formatted,
        "nested operator-owned scalar subquery formatting should remain idempotent"
    );
}

#[test]
fn format_sql_view_header_indents_cte_body_one_level_deeper() {
    let input = r#"CREATE OR REPLACE VIEW qt_fmt_emp_v AS
WITH dept_path (dept_id, parent_dept_id, dept_path_txt, lvl) AS (
SELECT dept_id, parent_dept_id, dept_path_txt, lvl
FROM dept
)
SELECT dept_id
FROM dept_path;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let with_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WITH dept_path"))
        .unwrap_or(0);
    let cte_select_idx = lines
        .iter()
        .enumerate()
        .skip(with_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SELECT")
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let main_select_idx = lines
        .iter()
        .enumerate()
        .skip(with_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SELECT dept_id")
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    assert_eq!(
        indent(lines[with_idx]),
        4,
        "VIEW body WITH should be one level deeper than the CREATE VIEW header, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[cte_select_idx]),
        8,
        "CTE body SELECT should be one level deeper than the WITH header under CREATE VIEW, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[main_select_idx]),
        4,
        "main SELECT after CTE should stay on the CREATE VIEW body depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_view_header_indents_plain_select_body_one_level_deeper() {
    let input = r#"CREATE OR REPLACE VIEW qt_fmt_emp_v AS
SELECT dept_id
FROM dept;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let select_idx = lines
        .iter()
        .position(|line| line.trim_start() == "SELECT dept_id")
        .unwrap_or(0);
    let from_idx = lines
        .iter()
        .position(|line| line.trim_start() == "FROM dept;")
        .unwrap_or(0);

    assert_eq!(
        indent(lines[select_idx]),
        4,
        "SELECT under CREATE VIEW ... AS should be indented one level deeper than the header, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[from_idx]),
        4,
        "FROM under CREATE VIEW ... AS should stay on the view body depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_ctas_header_indents_cte_body_one_level_deeper() {
    let input = r#"CREATE TABLE qt_fmt_emp_t AS
WITH dept_path (dept_id, parent_dept_id, dept_path_txt, lvl) AS (
SELECT dept_id, parent_dept_id, dept_path_txt, lvl
FROM dept
)
SELECT dept_id
FROM dept_path;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let with_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WITH dept_path"))
        .unwrap_or(0);
    let cte_select_idx = lines
        .iter()
        .enumerate()
        .skip(with_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SELECT")
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let main_select_idx = lines
        .iter()
        .enumerate()
        .skip(with_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SELECT dept_id")
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    assert_eq!(
        indent(lines[with_idx]),
        4,
        "CTAS body WITH should be one level deeper than the CREATE TABLE header, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[cte_select_idx]),
        8,
        "CTE body SELECT should be one level deeper than the WITH header under CTAS, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[main_select_idx]),
        4,
        "main SELECT after CTAS CTE should stay on the CREATE TABLE body depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_ctas_header_indents_plain_select_body_one_level_deeper() {
    let input = r#"CREATE TABLE qt_fmt_emp_t AS
SELECT dept_id
FROM dept;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);
    let lines: Vec<&str> = formatted.lines().collect();
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ').count();

    let select_idx = lines
        .iter()
        .position(|line| line.trim_start() == "SELECT dept_id")
        .unwrap_or(0);
    let from_idx = lines
        .iter()
        .position(|line| line.trim_start() == "FROM dept;")
        .unwrap_or(0);

    assert_eq!(
        indent(lines[select_idx]),
        4,
        "SELECT under CREATE TABLE ... AS should be indented one level deeper than the header, got:\n{}",
        formatted
    );
    assert_eq!(
        indent(lines[from_idx]),
        4,
        "FROM under CREATE TABLE ... AS should stay on the CTAS body depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_type_body_keeps_member_function_header_on_one_line() {
    let input = r#"CREATE OR REPLACE TYPE BODY money_t AS
    MEMBER
    FUNCTION to_string RETURN VARCHAR2 IS
    BEGIN
        RETURN TO_CHAR (self.amount, 'FM999,999,999,990.00') || ' ' || self.currency;
    END to_string;
END;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("MEMBER FUNCTION to_string RETURN VARCHAR2 IS"),
        "TYPE BODY member function header should stay on one line, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("MEMBER\n    FUNCTION to_string"),
        "TYPE BODY member function header must not split MEMBER and FUNCTION across lines, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_type_body_keeps_constructor_function_header_on_one_line() {
    let input = r#"CREATE OR REPLACE TYPE BODY money_t AS
    CONSTRUCTOR
    FUNCTION money_t RETURN SELF AS RESULT IS
    BEGIN
        RETURN;
    END;
END;"#;
    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("CONSTRUCTOR FUNCTION money_t RETURN SELF AS RESULT IS"),
        "TYPE BODY constructor function header should stay on one line, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("CONSTRUCTOR\n    FUNCTION money_t"),
        "TYPE BODY constructor function header must not split CONSTRUCTOR and FUNCTION across lines, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_auto_formatting_keeps_nested_in_subquery_select_indentation() {
    let input = r#"select 1
from a
where b in (
        select 1
        from a
        where b in (
                select 1
                from a
            )
            and 2
    );"#;

    let expected = r#"SELECT 1
FROM a
WHERE b IN (
        SELECT 1
        FROM a
        WHERE b IN (
                SELECT 1
                FROM a
            )
            AND 2
    );"#;

    let formatted = SqlEditorWidget::format_for_auto_formatting(input, false);
    assert_eq!(
        formatted, expected,
        "nested IN subquery indentation must stay stable, got:\n{}",
        formatted
    );
}

#[test]
fn format_mariadb_test1_keeps_inline_over_body_and_on_duplicate_function_args_nested() {
    let source = load_mariadb_test_file("test1.txt");
    assert!(
        !source.is_empty(),
        "test_mysql/test1.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        &source,
        crate::db::connection::DatabaseType::MySQL,
    );
    let lines: Vec<&str> = formatted.lines().collect();

    let over_idx = find_line_starting_with(&lines, "DENSE_RANK() OVER (").unwrap_or(0);
    let order_idx = find_line_starting_with(&lines, "ORDER BY ob.total_usd DESC").unwrap_or(0);
    let on_duplicate_idx =
        find_line_starting_with(&lines, "ON DUPLICATE KEY UPDATE dept_name = CONCAT(")
            .expect("ON DUPLICATE KEY UPDATE owner line");
    let values_idx =
        find_line_starting_with(&lines, "VALUES(dept_name),").expect("CONCAT argument line");
    let literal_idx = find_line_starting_with(&lines, "' / touched'").expect("CONCAT literal line");
    let concat_close_idx = lines
        .iter()
        .enumerate()
        .skip(literal_idx + 1)
        .find(|(_, line)| line.trim_start() == "),")
        .map(|(idx, _)| idx)
        .expect("CONCAT close line");
    let over_close_idx = lines
        .iter()
        .enumerate()
        .skip(order_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with(") AS global_rank"))
        .map(|(idx, _)| idx)
        .expect("analytic OVER close line");

    assert!(
        leading_spaces(lines[order_idx]) > leading_spaces(lines[over_idx]),
        "inline OVER (...) body should indent deeper than the OVER owner line, got:\n{}",
        formatted
    );
    assert!(
        leading_spaces(lines[values_idx]) > leading_spaces(lines[on_duplicate_idx]),
        "CONCAT argument inside ON DUPLICATE KEY UPDATE should stay nested under the function owner, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[literal_idx]),
        leading_spaces(lines[values_idx]),
        "sibling CONCAT arguments should share the same function-body depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[concat_close_idx]),
        leading_spaces(lines[on_duplicate_idx]),
        "CONCAT close line should realign with the ON DUPLICATE KEY UPDATE owner depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[over_close_idx]),
        leading_spaces(lines[over_idx]),
        "analytic OVER close line should realign with the owner depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_mariadb_test2_keeps_labeled_main_block_and_inline_over_body_nested() {
    let source = load_mariadb_test_file("test2.txt");
    assert!(
        !source.is_empty(),
        "test_mysql/test2.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        &source,
        crate::db::connection::DatabaseType::MySQL,
    );
    let lines: Vec<&str> = formatted.lines().collect();

    let main_block_idx = find_line_starting_with(&lines, "main_block: BEGIN").unwrap_or(0);
    let create_proc_idx =
        find_line_starting_with(&lines, "CREATE PROCEDURE sp_run_parser_killer()").unwrap_or(0);
    let over_idx =
        find_line_starting_with(&lines, "ROW_NUMBER() OVER (").expect("ROW_NUMBER OVER owner line");
    let order_idx = find_line_starting_with(&lines, "ORDER BY weight_sum DESC,")
        .expect("ROW_NUMBER ORDER BY line");
    let on_duplicate_idx = find_line_starting_with(
        &lines,
        "ON DUPLICATE KEY UPDATE node_name = VALUES(node_name),",
    )
    .expect("ON DUPLICATE KEY UPDATE line");
    let over_close_idx = lines
        .iter()
        .enumerate()
        .skip(order_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with(") AS rn"))
        .map(|(idx, _)| idx)
        .expect("ROW_NUMBER OVER close line");

    assert!(
        lines[main_block_idx].trim_start() == "main_block: BEGIN",
        "main_block label should stay attached to BEGIN on its own line, got:\n{}",
        formatted
    );
    assert!(
        main_block_idx > create_proc_idx,
        "main_block label should not collapse onto the CREATE PROCEDURE header, got:\n{}",
        formatted
    );
    assert!(
        lines[on_duplicate_idx]
            .trim_start()
            .starts_with("ON DUPLICATE KEY UPDATE node_name = VALUES(node_name),"),
        "ON DUPLICATE KEY UPDATE should keep MySQL VALUES() tight in test2, got:\n{}",
        formatted
    );
    assert!(
        leading_spaces(lines[order_idx]) > leading_spaces(lines[over_idx]),
        "inline OVER (...) body should indent deeper than the OVER owner line, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[over_close_idx]),
        leading_spaces(lines[over_idx]),
        "ROW_NUMBER OVER close line should realign with the owner depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_mariadb_test3_keeps_labeled_begin_blocks_and_inline_over_body_nested() {
    let source = load_mariadb_test_file("test3.txt");
    assert!(
        !source.is_empty(),
        "test_mysql/test3.txt should not be empty"
    );

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        &source,
        crate::db::connection::DatabaseType::MySQL,
    );
    let lines: Vec<&str> = formatted.lines().collect();

    let main_block_idx = find_line_starting_with(&lines, "main_block: BEGIN").unwrap_or(0);
    let nested_block_idx = find_line_starting_with(&lines, "nested_block: BEGIN").unwrap_or(0);
    let create_proc_idx =
        find_line_starting_with(&lines, "CREATE PROCEDURE sp_run_ultra_final_boss()").unwrap_or(0);
    let over_idx = find_line_starting_with(&lines, "ROW_NUMBER() OVER (")
        .expect("owner_ranked ROW_NUMBER OVER owner line");
    let order_idx = find_line_starting_with(&lines, "ORDER BY owner_weighted DESC,")
        .expect("owner_ranked ORDER BY line");
    let on_duplicate_idx = find_line_starting_with(
        &lines,
        "ON DUPLICATE KEY UPDATE node_name = VALUES(node_name),",
    )
    .expect("first ON DUPLICATE KEY UPDATE line");
    let summary_rank_idx = find_line_starting_with(&lines, "`rank` = VALUES(`rank`),")
        .expect("summary rank VALUES() line");
    let over_close_idx = lines
        .iter()
        .enumerate()
        .skip(order_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with(") AS rn"))
        .map(|(idx, _)| idx)
        .expect("owner_ranked OVER close line");
    let second_over_idx = lines
        .iter()
        .enumerate()
        .skip(over_close_idx + 1)
        .find(|(_, line)| line.trim_start() == "ROW_NUMBER() OVER (")
        .map(|(idx, _)| idx)
        .expect("second ROW_NUMBER OVER owner line");
    let second_order_idx = lines
        .iter()
        .enumerate()
        .skip(second_over_idx + 1)
        .find(|(_, line)| {
            line.trim_start()
                .starts_with("ORDER BY weighted_minutes DESC,")
        })
        .map(|(idx, _)| idx)
        .expect("second ROW_NUMBER ORDER BY line");
    let second_over_close_idx = lines
        .iter()
        .enumerate()
        .skip(second_order_idx + 1)
        .find(|(_, line)| line.trim_start().starts_with(") AS rn"))
        .map(|(idx, _)| idx)
        .expect("second ROW_NUMBER OVER close line");

    assert!(
        lines[main_block_idx].trim_start() == "main_block: BEGIN",
        "top-level label should stay attached to BEGIN on its own line, got:\n{}",
        formatted
    );
    assert!(
        main_block_idx > create_proc_idx,
        "top-level label should not collapse onto the CREATE PROCEDURE header, got:\n{}",
        formatted
    );
    assert!(
        lines[nested_block_idx].trim_start() == "nested_block: BEGIN",
        "nested label should stay attached to BEGIN on its own line, got:\n{}",
        formatted
    );
    assert!(
        lines[on_duplicate_idx]
            .trim_start()
            .starts_with("ON DUPLICATE KEY UPDATE node_name = VALUES(node_name),"),
        "first ON DUPLICATE KEY UPDATE should keep MySQL VALUES() tight in test3, got:\n{}",
        formatted
    );
    assert!(
        lines[summary_rank_idx].trim_start() == "`rank` = VALUES(`rank`),",
        "summary upsert should keep backticked rank reference as tight MySQL VALUES() call, got:\n{}",
        formatted
    );
    assert!(
        leading_spaces(lines[order_idx]) > leading_spaces(lines[over_idx]),
        "inline OVER (...) body should indent deeper than the OVER owner line, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[over_close_idx]),
        leading_spaces(lines[over_idx]),
        "first ROW_NUMBER OVER close line should realign with the owner depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[second_over_close_idx]),
        leading_spaces(lines[second_over_idx]),
        "second ROW_NUMBER OVER close line should realign with the owner depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_count_star_tight_in_plain_and_analytic_calls() {
    let input = r#"SELECT COUNT(*),
    COUNT(*) OVER (PARTITION BY deptno)
FROM emp
HAVING COUNT(*) > 1;"#;

    let formatted = SqlEditorWidget::format_sql_basic(input);

    assert!(
        formatted.contains("COUNT(*)"),
        "COUNT(*) should stay tight without an extra space before the opening paren, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("COUNT (*)"),
        "formatter should not insert a space in COUNT(*), got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_sum_case_close_aligned_with_owner_depth() {
    let input = r#"CREATE PROCEDURE sp_status_counts(
    OUT p_done_cnt INT,
    OUT p_running_cnt INT
)
BEGIN
    SELECT SUM(
            CASE
                WHEN status_code = 'DONE' THEN 1
                ELSE 0
            END
),
        SUM(
            CASE
                WHEN status_code = 'RUNNING' THEN 1
                ELSE 0
            END
        )
    INTO p_done_cnt,
        p_running_cnt
    FROM task;
END$$"#;

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        input,
        crate::db::connection::DatabaseType::MySQL,
    );
    let lines: Vec<&str> = formatted.lines().collect();
    let select_idx = find_line_starting_with(&lines, "SELECT").expect("SELECT line");
    let first_sum_idx = lines
        .iter()
        .enumerate()
        .skip(select_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SUM(")
        .map(|(idx, _)| idx)
        .expect("first SUM owner line");
    let close_idx = lines
        .iter()
        .enumerate()
        .skip(first_sum_idx + 1)
        .find(|(_, line)| line.trim() == "),")
        .map(|(idx, _)| idx)
        .expect("SUM close line");
    let second_sum_idx = lines
        .iter()
        .enumerate()
        .skip(close_idx.saturating_add(1))
        .find(|(_, line)| line.trim_start() == "SUM(")
        .map(|(idx, _)| idx)
        .expect("second SUM owner line");

    assert_eq!(
        leading_spaces(lines[first_sum_idx]),
        leading_spaces(lines[select_idx]).saturating_add(4),
        "first multiline SUM should be promoted to SELECT-list body depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[second_sum_idx]),
        leading_spaces(lines[first_sum_idx]),
        "SUM siblings should share the same SELECT-list body depth, got:\n{}",
        formatted
    );

    assert_eq!(
        leading_spaces(lines[close_idx]),
        leading_spaces(lines[first_sum_idx]),
        "MariaDB multiline SUM close line should realign with the SUM owner depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mysql_multiline_sum_siblings_on_shared_select_list_depth() {
    let input = r#"CREATE PROCEDURE sp_collect_status_counts(
    OUT p_done_cnt INT,
    OUT p_running_cnt INT,
    OUT p_other_cnt INT
)
BEGIN
    SELECT SUM(
        CASE
            WHEN status_code = 'DONE' THEN 1
            ELSE 0
        END
    ),
        SUM(
            CASE
                WHEN status_code = 'RUNNING' THEN 1
                ELSE 0
            END
        ),
        SUM(
            CASE
                WHEN status_code NOT IN ('DONE', 'RUNNING') THEN 1
                ELSE 0
            END
        )
    INTO p_done_cnt,
        p_running_cnt,
        p_other_cnt
    FROM run_case;
END$$"#;

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        input,
        crate::db::connection::DatabaseType::MySQL,
    );
    let lines: Vec<&str> = formatted.lines().collect();
    let select_idx = find_line_starting_with(&lines, "SELECT").expect("SELECT line");
    let sum_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start() == "SUM(")
        .map(|(idx, _)| idx)
        .collect();

    assert_eq!(
        sum_indices.len(),
        3,
        "expected three SUM owners in the formatted output, got:\n{}",
        formatted
    );
    assert!(
        !formatted.contains("SELECT SUM("),
        "first SUM owner should be rendered on the SELECT-list body line, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[sum_indices[0]]),
        leading_spaces(lines[select_idx]).saturating_add(4),
        "first SUM should use SELECT-list body depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[sum_indices[1]]),
        leading_spaces(lines[sum_indices[0]]),
        "second SUM should align with the first SUM owner depth, got:\n{}",
        formatted
    );
    assert_eq!(
        leading_spaces(lines[sum_indices[2]]),
        leading_spaces(lines[sum_indices[0]]),
        "third SUM should align with the first SUM owner depth, got:\n{}",
        formatted
    );
}

#[test]
fn format_sql_keeps_mariadb_call_scalar_subquery_arguments_on_shared_call_depth() {
    let input = r#"CALL sp_assert_eq_bigint(
    (
        SELECT COUNT(*)
        FROM task_log
        ), (
            SELECT COALESCE(SUM(log_count), 0)
            FROM monthly_rollup
                ), 'rollup log_count sum mismatch'
);"#;

    let formatted = SqlEditorWidget::format_sql_basic_for_db_type(
        input,
        crate::db::connection::DatabaseType::MySQL,
    );
    let expected = r#"CALL sp_assert_eq_bigint(
    (
        SELECT COUNT(*)
        FROM task_log
    ),
    (
        SELECT COALESCE(SUM(log_count), 0)
        FROM monthly_rollup
    ),
    'rollup log_count sum mismatch'
);"#;

    assert_eq!(
        formatted, expected,
        "CALL argument siblings should return to the shared call body depth after each parenthesized scalar subquery"
    );
    assert_eq!(
        SqlEditorWidget::format_sql_basic_for_db_type(
            &formatted,
            crate::db::connection::DatabaseType::MySQL,
        ),
        expected,
        "MariaDB CALL scalar-subquery argument formatting should remain idempotent"
    );
}
