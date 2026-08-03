use super::*;
use std::fs;
use std::path::PathBuf;

/// Helper to extract statements from ScriptItems
fn get_statements(items: &[ScriptItem]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect()
}

fn load_query_test_file(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("test");
    path.push(name);
    fs::read_to_string(path).unwrap_or_default()
}

fn load_mariadb_query_test_file(name: &str) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for dir in ["test_mariadb", "test_mysql"] {
        let path = manifest_dir.join(dir).join(name);
        if let Ok(contents) = fs::read_to_string(path) {
            return contents;
        }
    }
    String::new()
}

#[derive(Clone, Copy)]
struct StatementCursorSample<'a> {
    label: &'a str,
    anchor: &'a str,
    cursor_offset: usize,
    preferred_db_type: Option<crate::db::connection::DatabaseType>,
    expected_start: &'a str,
    expected_contains: &'a [&'a str],
    expected_excludes: &'a [&'a str],
}

fn assert_statement_cursor_sample(sql: &str, sample: StatementCursorSample<'_>) {
    let anchor_start = sql.find(sample.anchor).unwrap_or_else(|| {
        panic!(
            "expected anchor `{}` for sample `{}`",
            sample.anchor, sample.label
        )
    });
    let cursor = anchor_start
        .saturating_add(sample.cursor_offset)
        .min(sql.len());
    let statement =
        QueryExecutor::statement_at_cursor_for_db_type(sql, cursor, sample.preferred_db_type)
            .unwrap_or_else(|| {
                panic!("expected statement at cursor for sample `{}`", sample.label)
            });

    assert!(
        statement.trim_start().starts_with(sample.expected_start),
        "sample `{}` should resolve statement starting with `{}` but got:\n{}",
        sample.label,
        sample.expected_start,
        statement
    );

    for needle in sample.expected_contains {
        assert!(
            statement.contains(needle),
            "sample `{}` should keep `{}` in the selected statement, got:\n{}",
            sample.label,
            needle,
            statement
        );
    }

    for needle in sample.expected_excludes {
        assert!(
            !statement.contains(needle),
            "sample `{}` should not leak `{}` into the selected statement, got:\n{}",
            sample.label,
            needle,
            statement
        );
    }
}

#[test]
fn test_statement_bounds_at_cursor_ignores_string_literal_with_previous_statement_text() {
    let sql = "SELECT 1 FROM dual;
SELECT 'SELECT 1 FROM dual' AS txt FROM dual;";
    let cursor = sql.rfind("txt").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for second statement");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.contains("AS txt FROM dual"),
        "expected second statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_ignores_comment_with_previous_statement_text() {
    let sql = "SELECT 1 FROM dual;
/* SELECT 1 FROM dual */
SELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for statement after comment");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("SELECT 2 FROM dual"),
        "expected final statement, got: {statement}"
    );
}

#[test]
fn test_statement_at_cursor_samples_large_oracle_regression_scripts() {
    let mega_torture = load_query_test_file("mega_torture.txt");
    assert_statement_cursor_sample(
        &mega_torture,
        StatementCursorSample {
            label: "mega torture trigger",
            anchor: "CREATE OR REPLACE TRIGGER oqt_trg_test_bi",
            cursor_offset: 24,
            preferred_db_type: None,
            expected_start: "CREATE OR REPLACE TRIGGER oqt_trg_test_bi",
            expected_contains: &["BEFORE INSERT", "INSERT INTO oqt_t_log"],
            expected_excludes: &["END oqt_mega_pkg;", "SHOW ERRORS PACKAGE BODY"],
        },
    );
    assert_statement_cursor_sample(
        &mega_torture,
        StatementCursorSample {
            label: "mega torture execution block",
            anchor: "oqt_mega_pkg.seed(30)",
            cursor_offset: 8,
            preferred_db_type: None,
            expected_start: "BEGIN",
            expected_contains: &["oqt_mega_pkg.seed(30);"],
            expected_excludes: &["CREATE OR REPLACE TRIGGER oqt_trg_test_bi"],
        },
    );
    assert_statement_cursor_sample(
        &mega_torture,
        StatementCursorSample {
            label: "mega torture trailing summary query",
            anchor: "SELECT grp,",
            cursor_offset: 7,
            preferred_db_type: None,
            expected_start: "SELECT grp,",
            expected_contains: &["FROM oqt_t_test"],
            expected_excludes: &["PRINT v_rc", "BEGIN\n  oqt_mega_pkg.open_rc"],
        },
    );

    let test15 = load_query_test_file("test15.sql");
    assert_statement_cursor_sample(
        &test15,
        StatementCursorSample {
            label: "test15 package body",
            anchor: "payload = q'[dynamic ; payload / still string]'",
            cursor_offset: 11,
            preferred_db_type: None,
            expected_start: "CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg",
            expected_contains: &[
                "payload = q'[dynamic ; payload / still string]'",
                "END qt_splitter_pkg",
            ],
            expected_excludes: &["COMMENT ON TABLE qt_splitter_boss IS"],
        },
    );
    assert_statement_cursor_sample(
        &test15,
        StatementCursorSample {
            label: "test15 comment statement",
            anchor: "COMMENT ON TABLE qt_splitter_boss IS",
            cursor_offset: 18,
            preferred_db_type: None,
            expected_start: "COMMENT ON TABLE qt_splitter_boss IS",
            expected_contains: &["splitter final boss ; comment / text"],
            expected_excludes: &["CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg"],
        },
    );

    let test16 = load_query_test_file("test16.sql");
    assert_statement_cursor_sample(
        &test16,
        StatementCursorSample {
            label: "test16 package body",
            anchor: "touch_row(p_id, 'AMOUNT>=1000;DONE-CANDIDATE');",
            cursor_offset: 18,
            preferred_db_type: None,
            expected_start: "CREATE OR REPLACE PACKAGE BODY qt_splitter_ultimate_pkg",
            expected_contains: &[
                "touch_row(p_id, 'AMOUNT>=1000;DONE-CANDIDATE');",
                "END qt_splitter_ultimate_pkg",
            ],
            expected_excludes: &["COMMENT ON TABLE qt_splitter_ultimate IS"],
        },
    );
    assert_statement_cursor_sample(
        &test16,
        StatementCursorSample {
            label: "test16 comment statement",
            anchor: "COMMENT ON TABLE qt_splitter_ultimate IS",
            cursor_offset: 19,
            preferred_db_type: None,
            expected_start: "COMMENT ON TABLE qt_splitter_ultimate IS",
            expected_contains: &["ultimate splitter boss ; / table comment"],
            expected_excludes: &["CREATE OR REPLACE PACKAGE BODY qt_splitter_ultimate_pkg"],
        },
    );

    let test20 = load_query_test_file("test20.sql");
    assert_statement_cursor_sample(
        &test20,
        StatementCursorSample {
            label: "test20 final audit preview query",
            anchor: "AS msg_preview",
            cursor_offset: 4,
            preferred_db_type: None,
            expected_start: "SELECT audit_id, module_name, action_name, SUBSTR(message_text, 1, 120) AS msg_preview",
            expected_contains: &["FROM qt_fb_audit"],
            expected_excludes: &["FROM qt_fb_view", "p_module => 'final_validation'"],
        },
    );

    let test21 = load_query_test_file("test21.sql");
    assert_statement_cursor_sample(
        &test21,
        StatementCursorSample {
            label: "test21 final error preview query",
            anchor: "AS err_msg_preview",
            cursor_offset: 6,
            preferred_db_type: None,
            expected_start: "SELECT err_id,",
            expected_contains: &["FROM qt_x_err_log", "AS err_msg_preview"],
            expected_excludes: &["FROM qt_x_audit", "p_module => 'final_validation'"],
        },
    );
}

#[test]
fn test_statement_at_cursor_samples_large_mariadb_regression_scripts() {
    let mysql_db = Some(crate::db::connection::DatabaseType::MySQL);

    let test1 = load_mariadb_query_test_file("test1.txt");
    assert_statement_cursor_sample(
        &test1,
        StatementCursorSample {
            label: "mariadb final boss use command",
            anchor: "USE qt_mysql_final_boss;",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "USE qt_mysql_final_boss",
            expected_contains: &[],
            expected_excludes: &["CREATE TABLE dept"],
        },
    );
    assert_statement_cursor_sample(
        &test1,
        StatementCursorSample {
            label: "mariadb final boss hash comment gap",
            anchor: "# line comment torture: ; ; ; DELIMITER $$ should not matter here",
            cursor_offset: 3,
            preferred_db_type: mysql_db,
            expected_start: "CREATE TABLE dept",
            expected_contains: &["CONSTRAINT fk_dept_parent"],
            expected_excludes: &["/*!80000 SET @versioned_comment_executed = 1 */"],
        },
    );
    assert_statement_cursor_sample(
        &test1,
        StatementCursorSample {
            label: "mariadb final boss procedure body",
            anchor: "JOIN JSON_TABLE(",
            cursor_offset: 7,
            preferred_db_type: mysql_db,
            expected_start: "CREATE PROCEDURE sp_run_final_boss ()",
            expected_contains: &["JOIN JSON_TABLE(", "WINDOW", "COMMIT;"],
            expected_excludes: &["CALL sp_run_final_boss();"],
        },
    );
    assert_statement_cursor_sample(
        &test1,
        StatementCursorSample {
            label: "mariadb final boss trailing summary query",
            anchor: "'PASS' AS status",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "SELECT",
            expected_contains: &["'PASS' AS status", "expected_duplicate_errors"],
            expected_excludes: &["CALL sp_run_final_boss();"],
        },
    );

    let test2 = load_mariadb_query_test_file("test2.txt");
    assert_statement_cursor_sample(
        &test2,
        StatementCursorSample {
            label: "mariadb parser killer use command",
            anchor: "USE qt_mysql_parser_killer;",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "USE qt_mysql_parser_killer",
            expected_contains: &[],
            expected_excludes: &["CREATE TABLE node"],
        },
    );
    assert_statement_cursor_sample(
        &test2,
        StatementCursorSample {
            label: "mariadb parser killer delimiter command",
            anchor: "DELIMITER //\n\nCREATE PROCEDURE sp_run_parser_killer ()",
            cursor_offset: 0,
            preferred_db_type: mysql_db,
            expected_start: "DELIMITER //",
            expected_contains: &[],
            expected_excludes: &["CREATE PROCEDURE sp_run_parser_killer ()"],
        },
    );
    assert_statement_cursor_sample(
        &test2,
        StatementCursorSample {
            label: "mariadb parser killer procedure body",
            anchor: "ORDER BY weight_sum DESC, owner_name",
            cursor_offset: 10,
            preferred_db_type: mysql_db,
            expected_start: "CREATE PROCEDURE sp_run_parser_killer ()",
            expected_contains: &["WITH owner_score AS (", "ROW_NUMBER() OVER (", "COMMIT;"],
            expected_excludes: &["CALL sp_run_parser_killer();"],
        },
    );
    assert_statement_cursor_sample(
        &test2,
        StatementCursorSample {
            label: "mariadb parser killer final result query",
            anchor: "ORDER BY result_key;",
            cursor_offset: 10,
            preferred_db_type: mysql_db,
            expected_start: "SELECT",
            expected_contains: &["FROM agg_result", "ORDER BY result_key"],
            expected_excludes: &["FROM error_log"],
        },
    );

    let test3 = load_mariadb_query_test_file("test3.txt");
    assert_statement_cursor_sample(
        &test3,
        StatementCursorSample {
            label: "mariadb ultra final boss use command",
            anchor: "USE qt_mysql_ultra_final_boss;",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "USE qt_mysql_ultra_final_boss",
            expected_contains: &[],
            expected_excludes: &["CREATE TABLE stage_node"],
        },
    );
    assert_statement_cursor_sample(
        &test3,
        StatementCursorSample {
            label: "mariadb ultra final boss delimiter command",
            anchor: "DELIMITER //\n\nCREATE PROCEDURE sp_run_ultra_final_boss ()",
            cursor_offset: 0,
            preferred_db_type: mysql_db,
            expected_start: "DELIMITER //",
            expected_contains: &[],
            expected_excludes: &["CREATE PROCEDURE sp_run_ultra_final_boss ()"],
        },
    );
    assert_statement_cursor_sample(
        &test3,
        StatementCursorSample {
            label: "mariadb ultra final boss procedure body",
            anchor: "WINDOW",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "CREATE PROCEDURE sp_run_ultra_final_boss ()",
            expected_contains: &["WINDOW", "running owner weighted max", "COMMIT;"],
            expected_excludes: &["CALL sp_run_ultra_final_boss();"],
        },
    );
    assert_statement_cursor_sample(
        &test3,
        StatementCursorSample {
            label: "mariadb ultra final boss final summary query",
            anchor: "'PASS' AS status",
            cursor_offset: 2,
            preferred_db_type: mysql_db,
            expected_start: "SELECT",
            expected_contains: &["'PASS' AS status", "top_run_id"],
            expected_excludes: &["CALL sp_run_ultra_final_boss();"],
        },
    );
    assert_statement_cursor_sample(
        &test3,
        StatementCursorSample {
            label: "mariadb ultra final boss qa summary query",
            anchor: "ORDER BY `rank`, summary_key;",
            cursor_offset: 10,
            preferred_db_type: mysql_db,
            expected_start: "SELECT",
            expected_contains: &["FROM qa_summary", "ORDER BY `rank`, summary_key"],
            expected_excludes: &["FROM error_log"],
        },
    );
}

#[test]
fn test_split_script_items_test12_if_alias_regression() {
    let sql = load_query_test_file("test12.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 5,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 46,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| stmt.contains("WITH\nIF AS (")),
        "WITH if CTE statement should stay intact: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.contains("MERGE INTO qt_if_base")),
        "MERGE statement should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test13_keyword_alias_regression() {
    let sql = load_query_test_file("test13.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 5,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 49,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.contains("FROM qt_kw_base trim")),
        "trim alias statement should stay intact: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.contains("WITH trim AS (")),
        "trim CTE statement should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test14_deep_monster_view_regression() {
    let sql = load_query_test_file("test14.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 3,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 13,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.contains("CREATE OR REPLACE VIEW qt_depth_monster_v")),
        "deep monster view should stay intact: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.contains("FROM qt_depth_monster_v trim")),
        "final execution query should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test15_nested_q_quote_package_body_regression() {
    let sql = load_query_test_file("test15.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 0,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 20,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg")
                && stmt.contains("payload = q'[dynamic ; payload / still string]'")
                && stmt.contains("END qt_splitter_pkg")
        }),
        "package body with nested q-quote should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_splitter_proc")
                && stmt.contains("END qt_splitter_proc")
        }),
        "stored procedure should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("BEGIN")
                && stmt.contains("qt_splitter_proc(10);")
                && stmt.contains("END LOOP;")
        }),
        "final anonymous block should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test16_final_ultimate_boss_regression() {
    let sql = load_query_test_file("test16.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 4,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 35,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_splitter_ultimate_proc")
                && stmt.contains("v_rendered := q'[fallback ; / ]'")
                && stmt.contains("UPDATE qt_splitter_ultimate")
                && stmt.contains("END qt_splitter_ultimate_proc")
        }),
        "standalone procedure with nested block should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_splitter_ultimate_pkg")
                && stmt.contains("touch_row(p_id, 'AMOUNT>=1000;DONE-CANDIDATE');")
                && stmt.contains("END qt_splitter_ultimate_pkg")
        }),
        "package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("WITH base_data AS") && stmt.contains("ORDER BY f.grp, f.rn, f.id")
        }),
        "WITH query should stay intact: {statements:?}"
    );
    assert!(
        matches!(
            items.first(),
            Some(ScriptItem::ToolCommand(ToolCommand::SetDefine {
                enabled: true,
                define_char: None
            }))
        ),
        "expected SET DEFINE ON tool command at the start: {items:?}"
    );
    assert!(
        matches!(
            items.last(),
            Some(ScriptItem::ToolCommand(ToolCommand::Prompt { text }))
                if text == "=== QT SPLITTER FINAL ULTIMATE BOSS END ==="
        ),
        "expected final PROMPT tool command at the end: {items:?}"
    );
}

#[test]
fn test_split_script_items_test17_execution_unit_final_boss_regression() {
    let sql = load_query_test_file("test17.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 1,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 23,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_split_pkg")
                && stmt.contains("q'{ | q2=/* not comment */ }'")
                && stmt.contains("END qt_split_pkg")
        }),
        "package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_split_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_split_proc")
        }),
        "standalone procedure should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("v_q1 := q'[")
                && stmt.contains("END lvl1;")
                && stmt.contains("END;")
        }),
        "lexical trap anonymous block should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT log_id,")
                && stmt.contains("SUBSTR(payload, 1, 120) AS payload_preview")
                && stmt.contains("ORDER BY log_id")
        }),
        "final log detail query should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test19_execution_unit_splitter_final_boss_regression() {
    let sql = load_query_test_file("test19.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 5,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 24,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_boss_pkg")
                && stmt.contains("g_body_trap CONSTANT VARCHAR2(32767) := q'~BODY-BEGIN")
                && stmt.contains("END qt_boss_pkg")
        }),
        "package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_boss_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_boss_proc")
        }),
        "standalone procedure should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("LOG DISTRIBUTION FAIL")
                && stmt.contains("v_lex_cnt")
                && stmt.trim_end().ends_with("END")
        }),
        "verification anonymous block should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT log_id,")
                && stmt.contains("SUBSTR(payload, 1, 120) AS payload_preview")
                && stmt.contains("ORDER BY log_id")
        }),
        "final payload preview query should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test20_execution_unit_splitter_regression() {
    let sql = load_query_test_file("test20.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 3,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 39,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_fb_pkg")
                && stmt.contains("g_last_message :=")
                && stmt.contains("END qt_fb_pkg")
        }),
        "package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE TRIGGER qt_fb_trg")
                && stmt.contains("negative salary is invalid; / ;")
                && stmt.trim_end().ends_with("END")
        }),
        "trigger body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("p_module => 'final_validation'")
                && stmt.contains("SELECT COUNT(*) INTO v_pipe_cnt")
                && stmt.contains("DBMS_OUTPUT.PUT_LINE(v_text)")
        }),
        "final validation block should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT audit_id, module_name, action_name")
                && stmt.contains("AS msg_preview")
                && stmt.contains("ORDER BY audit_id")
        }),
        "final audit preview query should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test21_execution_unit_splitter_regression() {
    let sql = load_query_test_file("test21.sql");
    let items = QueryExecutor::split_script_items(&sql);

    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();

    assert_eq!(
        tool_command_count, 3,
        "unexpected tool command split: {items:?}"
    );
    assert_eq!(
        statement_count, 47,
        "unexpected SQL statement split: {items:?}"
    );

    let statements = get_statements(&items);
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_x_util_pkg")
                && stmt.contains("fake package:")
                && stmt.contains("END qt_x_util_pkg")
        }),
        "utility package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_x_chaos_pkg")
                && stmt.contains("PROCEDURE execute_dynamic_hell")
                && stmt.contains("FINAL PACKAGE MESSAGE")
                && stmt.contains("END qt_x_chaos_pkg")
        }),
        "chaos package body should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("p_module => 'selection_killer'")
                && stmt.contains("SELECT COUNT(*) FROM qt_x_emp WHERE emp_name LIKE :x")
                && stmt.contains("END IF;")
        }),
        "selection execution killer block should stay intact: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT err_id,")
                && stmt.contains("AS err_msg_preview")
                && stmt.contains("ORDER BY err_id")
        }),
        "final error preview query should stay intact: {statements:?}"
    );
}

#[test]
fn test_split_script_items_test4_mariadb_full_script_keeps_delimiter_routine_boundaries() {
    let sql = load_mariadb_query_test_file("test4.txt");
    let items = QueryExecutor::split_script_items_for_db_type(
        &sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );

    let statements = get_statements(&items);
    let mysql_delimiters = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::ToolCommand(ToolCommand::MysqlDelimiter { delimiter }) => {
                Some(delimiter.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        mysql_delimiters,
        vec![";", "$$", ";"],
        "test4 should preserve top-level DELIMITER commands in order: {items:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE OR REPLACE VIEW v_employee_workload AS")),
        "view statement before DELIMITER $$ should remain independently split: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE TRIGGER bi_task_log")),
        "trigger after DELIMITER $$ should start its own execution unit: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE TRIGGER ai_task_log")),
        "second trigger should stay independently executable after END$$: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE FUNCTION fn_efficiency_band")),
        "function should stay independently executable after END$$: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE PROCEDURE sp_seed_monster_data")),
        "first procedure should stay independently executable after END$$: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.starts_with("CREATE PROCEDURE sp_build_monthly_rollup")),
        "second procedure should stay independently executable after END$$: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|stmt| stmt.trim_start().starts_with("CALL sp_seed_monster_data()")),
        "post-routine CALL should remain separated from the preceding END$$ block: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| stmt
            .trim_start()
            .starts_with("SELECT 'ALL ASSERTIONS PASSED' AS status")),
        "post-routine assertions/selects should remain independently executable: {statements:?}"
    );
}

#[test]
fn test_split_script_items_standalone_procedure_nested_block_followed_by_dml() {
    let sql = r#"CREATE OR REPLACE PROCEDURE nested_proc IS
BEGIN
    BEGIN
        NULL;
    END;

    UPDATE dual
       SET dummy = dummy;
END nested_proc;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(statements.len(), 2, "unexpected split: {statements:?}");
    assert!(
        statements[0].contains("UPDATE dual") && statements[0].contains("END nested_proc"),
        "procedure body should stay intact: {}",
        statements[0]
    );
    assert_eq!(statements[1].trim(), "SELECT 1 FROM dual");
}

#[test]
fn test_split_format_items_test15_nested_q_quote_package_body_regression() {
    let sql = load_query_test_file("test15.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    // UNIT 02, 05, 06, 07, 08, 14 and 15 are independently slash-terminated.
    assert_eq!(slash_count, 7, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_splitter_pkg")
                && stmt.contains("payload = q'[dynamic ; payload / still string]'")
                && stmt.contains("END qt_splitter_pkg")
        }),
        "package body with nested q-quote should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_splitter_proc")
                && stmt.contains("END qt_splitter_proc")
        }),
        "stored procedure should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .all(|stmt| stmt.trim() != "END qt_splitter_proc"),
        "orphan END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_split_format_items_test16_final_ultimate_boss_regression() {
    let sql = load_query_test_file("test16.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(slash_count, 20, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_splitter_ultimate_proc")
                && stmt.contains("AND t.\"COMMENT\" LIKE q'[%;%]'")
                && stmt.contains("v_rendered := q'[fallback ; / ]'")
                && stmt.contains("UPDATE qt_splitter_ultimate")
                && stmt.contains("END qt_splitter_ultimate_proc")
        }),
        "standalone procedure should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_splitter_ultimate_pkg")
                && stmt.contains("q'[dyn ; / -- '' ]'")
                && stmt.contains("q'[payload from merge_like ; / ]'")
                && stmt.contains("touch_row(p_id, 'AMOUNT>=1000;DONE-CANDIDATE');")
                && stmt.contains("END qt_splitter_ultimate_pkg")
        }),
        "package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .all(|stmt| stmt.trim() != "END qt_splitter_ultimate_proc"),
        "orphan END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_split_format_items_test17_execution_unit_final_boss_regression() {
    let sql = load_query_test_file("test17.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(slash_count, 10, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_split_pkg")
                && stmt.contains("q'{ | q2=/* not comment */ }'")
                && stmt.contains("END qt_split_pkg")
        }),
        "package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_split_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_split_proc")
        }),
        "standalone procedure should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("v_q1 := q'[")
                && stmt.contains("END lvl1;")
                && stmt.contains("END;")
        }),
        "lexical trap anonymous block should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .all(|stmt| stmt.trim() != "END qt_split_proc"),
        "orphan END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_split_format_items_test19_execution_unit_splitter_final_boss_regression() {
    let sql = load_query_test_file("test19.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(slash_count, 14, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_boss_pkg")
                && stmt.contains("g_body_trap CONSTANT VARCHAR2(32767) := q'~BODY-BEGIN")
                && stmt.contains("END qt_boss_pkg")
        }),
        "package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PROCEDURE qt_boss_proc")
                && stmt.contains("END LOOP outer_loop;")
                && stmt.contains("END qt_boss_proc")
        }),
        "standalone procedure should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("LOG DISTRIBUTION FAIL")
                && stmt.contains("v_lex_cnt")
                && stmt.trim_end().ends_with("END")
        }),
        "verification anonymous block should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements
            .iter()
            .all(|stmt| stmt.trim() != "END qt_boss_pkg"),
        "orphan package END label should not remain standalone in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().all(|stmt| stmt.trim() != "END qt_boss_proc"),
        "orphan procedure END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_split_format_items_test20_execution_unit_splitter_regression() {
    let sql = load_query_test_file("test20.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(slash_count, 15, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_fb_pkg")
                && stmt.contains("g_last_message :=")
                && stmt.contains("END qt_fb_pkg")
        }),
        "package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE TRIGGER qt_fb_trg")
                && stmt.contains("negative salary is invalid; / ;")
                && stmt.trim_end().ends_with("END")
        }),
        "trigger should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("p_module => 'final_validation'")
                && stmt.contains("SELECT COUNT(*) INTO v_pipe_cnt")
                && stmt.contains("DBMS_OUTPUT.PUT_LINE(v_text)")
        }),
        "final validation block should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().all(|stmt| stmt.trim() != "END qt_fb_pkg"),
        "orphan package END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_split_format_items_test21_execution_unit_splitter_regression() {
    let sql = load_query_test_file("test21.sql");
    let items = QueryExecutor::split_format_items(&sql);

    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(slash_count, 25, "unexpected slash split: {items:?}");

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_x_util_pkg")
                && stmt.contains("fake package:")
                && stmt.contains("END qt_x_util_pkg")
        }),
        "utility package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE OR REPLACE PACKAGE BODY qt_x_chaos_pkg")
                && stmt.contains("PROCEDURE execute_dynamic_hell")
                && stmt.contains("FINAL PACKAGE MESSAGE")
                && stmt.contains("END qt_x_chaos_pkg")
        }),
        "chaos package body should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("DECLARE")
                && stmt.contains("p_module => 'selection_killer'")
                && stmt.contains("SELECT COUNT(*) FROM qt_x_emp WHERE emp_name LIKE :x")
                && stmt.contains("END IF;")
        }),
        "selection execution killer block should stay intact in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().all(|stmt| stmt.trim() != "END qt_x_util_pkg"),
        "orphan utility package END label should not remain standalone in format splitter: {statements:?}"
    );
    assert!(
        statements.iter().all(|stmt| stmt.trim() != "END qt_x_chaos_pkg"),
        "orphan chaos package END label should not remain standalone in format splitter: {statements:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_clamps_non_boundary_utf8_offset() {
    let sql = "SELECT 1 FROM dual;\nSELECT 한글 AS txt FROM dual;";
    let utf8_start = sql
        .find('한')
        .expect("expected utf-8 anchor in second statement");
    let mid_char_cursor = utf8_start + 1;
    assert!(
        !sql.is_char_boundary(mid_char_cursor),
        "test requires a non-byte-boundary cursor to validate clamping"
    );

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, mid_char_cursor)
        .expect("expected statement bounds for UTF-8 cursor offset");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.contains("한글 AS txt"),
        "expected second statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_prefers_next_statement_on_boundary() {
    let sql = "SELECT 1 FROM dual;
SELECT 2 FROM dual;";
    let boundary_cursor = sql.find("SELECT 2").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, boundary_cursor)
        .expect("expected statement bounds at boundary cursor");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("SELECT 2 FROM dual"),
        "expected second statement at boundary, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_ignores_tool_command_line_inside_string_literal() {
    // A line that looks like a SQL*Plus command but sits inside a multi-line string
    // literal must not be mistaken for a standalone tool command.
    let sql = "SELECT 'x\nPROMPT hi\ny' FROM dual;\nSELECT 2 FROM dual;";
    let cursor = sql.find("PROMPT hi").unwrap() + 2;

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::Oracle),
    )
    .expect("expected statement bounds for cursor inside string literal");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("SELECT 'x"),
        "cursor inside a string literal must resolve to the enclosing statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_ignores_tool_command_line_inside_block_comment() {
    // A line that looks like a SQL*Plus command but sits inside a block comment must
    // not be executed as a tool command.
    let sql = "/*\nSET ECHO ON\n*/\nSELECT 1 FROM dual;";
    let cursor = sql.find("SET ECHO ON").unwrap() + 2;

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::Oracle),
    )
    .expect("expected statement bounds for cursor inside block comment");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        !statement.contains("SET ECHO ON") || statement.contains("SELECT 1"),
        "cursor inside a block comment must not resolve to the commented tool command, got: {statement}"
    );
    assert!(
        statement.trim_start().starts_with("SELECT 1"),
        "cursor inside a leading block comment should resolve to the following statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_standalone_tool_command_line() {
    // The standalone tool-command short-circuit must still work for genuine command
    // lines at a statement boundary (including right after a PL/SQL `/` terminator).
    let sql = "BEGIN NULL; END;\n/\nPROMPT hi\nSELECT 1 FROM dual;";
    let cursor = sql.find("PROMPT hi").unwrap() + 2;

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::Oracle),
    )
    .expect("expected statement bounds for standalone tool command line");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "PROMPT hi",
        "cursor on a genuine standalone tool command line must resolve to it, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_ignores_slash_line_inside_string_literal() {
    // A lone `/` line inside a multi-line string literal is not a SQL*Plus PL/SQL
    // terminator; the cursor must resolve to the enclosing statement.
    let sql = "SELECT 'x\n/\ny' FROM dual;\nSELECT 2 FROM dual;";
    let cursor = sql.find("\n/\n").unwrap() + 1;

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::Oracle),
    )
    .expect("expected statement bounds for `/` line inside string literal");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("SELECT 'x"),
        "`/` inside a string literal must resolve to the enclosing statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_ignores_slash_line_inside_block_comment() {
    // A lone `/` line inside a block comment is not a terminator either.
    let sql = "/*\n/\n*/\nSELECT 1 FROM dual;";
    let cursor = sql.find("\n/\n").unwrap() + 1;

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::Oracle),
    )
    .expect("expected statement bounds for `/` line inside block comment");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 1"),
        "`/` inside a block comment must resolve to the following statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_first_exec_without_semicolon() {
    let sql = "EXEC proc1\nEXEC proc2";
    // Cursor sits right after the first EXEC line's last token (before the newline).
    // EXEC is auto-terminated without a ';', so the inter-statement gap is just "\n";
    // the cursor must still resolve to the first statement, not the second.
    let cursor = "EXEC proc1".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds at end of first EXEC line");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "EXEC proc1",
        "cursor at end of first EXEC line must resolve to the first statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_first_exec_with_trailing_whitespace() {
    // Trailing spaces on the first EXEC line, reachable with the End key. The
    // cursor is still on the first statement's line, so it must resolve to the
    // first statement even though the gap to the next statement is whitespace.
    let sql = "EXEC proc1   \nEXEC proc2";
    let cursor = "EXEC proc1   ".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds in first EXEC line trailing whitespace");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "EXEC proc1",
        "cursor in first EXEC line trailing whitespace must resolve to the first statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_first_exec_with_trailing_block_comment() {
    // A block comment trailing the first EXEC on the same line belongs to that
    // statement, so a cursor inside it resolves to the first statement — unlike a
    // leading block comment on its own line, which attaches to the next statement.
    let sql = "EXEC proc1 /* note */\nEXEC proc2";
    let cursor = "EXEC proc1 /* no".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds inside trailing block comment");
    let statement = &sql[bounds.0..bounds.1];

    // The trailing comment is trimmed from the span, but the cursor must still
    // resolve to the first statement rather than EXEC proc2.
    assert_eq!(
        statement.trim(),
        "EXEC proc1",
        "cursor in first EXEC line trailing block comment must resolve to the first statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_blank_line_gap_prefers_next_statement() {
    // A cursor on a genuine blank line *between* two statements keeps preferring
    // the following statement (intentional, matches the routine-gap behavior).
    let sql = "EXEC proc1\n\nEXEC proc2";
    let cursor = "EXEC proc1\n".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds on blank line gap");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "EXEC proc2",
        "cursor on blank line between statements must resolve to the next statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_splits_around_tool_command_line() {
    let sql = "SELECT 1 FROM dual;\nPROMPT section\nSELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds after tool command line");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "expected final SELECT statement, got: {statement}"
    );
    assert!(
        !statement.contains("PROMPT"),
        "tool command line must not leak into SQL statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_splits_around_run_buffer_command_line() {
    let sql = "SELECT 1 FROM dual;\nRUN\nSELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds after RUN command line");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "expected final SELECT statement, got: {statement}"
    );
    assert!(
        !statement.contains("RUN"),
        "RUN command line must not leak into SQL statement bounds: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_splits_after_with_function_and_run_script_command() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\n@child.sql\nSELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds after @ script command");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "expected trailing SELECT statement, got: {statement}"
    );
    assert!(
        !statement.contains("@child.sql"),
        "run-script command line must not leak into SQL statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_with_function_end_label_and_trailing_ctes_attached() {
    let sql = "WITH
    FUNCTION calc_depth (p_id NUMBER) RETURN NUMBER IS v_depth NUMBER;

BEGIN
    SELECT MAX (LEVEL)
    INTO v_depth
    FROM org_tree
    START WITH parent_id IS NULL
    CONNECT BY PRIOR node_id = parent_id;
    RETURN v_depth;
END calc_depth;

recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS (
    SELECT node_id,
        parent_id,
        node_name,
        1 AS DEPTH,
        CAST (node_name AS VARCHAR2 (4000)) AS PATH
    FROM org_tree
    WHERE parent_id IS NULL
    UNION ALL
    SELECT t.node_id,
        t.parent_id,
        t.node_name,
        rt.DEPTH + 1,
        rt.PATH || ' > ' || t.node_name
    FROM org_tree t
    JOIN recursive_tree rt
        ON t.parent_id = rt.node_id
    WHERE rt.DEPTH < calc_depth (t.node_id)
),
    aggregated AS (
        SELECT parent_id,
            COUNT (*) AS child_count,
            MAX (DEPTH) AS max_depth,
            LISTAGG (node_name, ', ') WITHIN GROUP (ORDER BY node_name) AS children
        FROM recursive_tree
        WHERE DEPTH > 1
        GROUP BY parent_id
    )
SELECT rt.*,
    a.child_count,
    a.max_depth,
    a.children
FROM recursive_tree rt
LEFT JOIN aggregated a
    ON rt.node_id = a.parent_id
ORDER BY rt.PATH;
SELECT 2 FROM dual;";
    let cursor = sql.find("aggregated AS").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for WITH FUNCTION + trailing CTE query");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("WITH\n    FUNCTION calc_depth"),
        "statement bounds should start from WITH FUNCTION declaration: {statement}"
    );
    assert!(
        statement.contains("END calc_depth;"),
        "function end label should remain inside statement bounds: {statement}"
    );
    assert!(
        statement.contains("aggregated AS ("),
        "trailing CTE should remain inside statement bounds: {statement}"
    );
    assert!(
        statement.contains("ORDER BY rt.PATH"),
        "main query tail should remain inside statement bounds: {statement}"
    );
    assert!(
        !statement.contains("SELECT 2 FROM dual;"),
        "next statement must not leak into current statement bounds: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_multiline_alter_session_set_clause() {
    let sql = "ALTER SESSION\nSET NLS_DATE_FORMAT = 'YYYY-MM-DD';\nSELECT 1 FROM dual;";
    let cursor = sql.find("NLS_DATE_FORMAT").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected ALTER SESSION statement bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.contains("ALTER SESSION"),
        "ALTER SESSION header should be included: {statement}"
    );
    assert!(
        statement.contains("SET NLS_DATE_FORMAT"),
        "SET clause should remain part of ALTER SESSION: {statement}"
    );
    assert!(
        !statement.contains("SELECT 1 FROM dual"),
        "next statement should not be included: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_oracle_profile_connect_time_resource() {
    let sql = "CREATE PROFILE app_profile LIMIT
  SESSIONS_PER_USER 3
  CONN UNLIMITED
  IDLE_TIME 30;";
    let cursor = sql.find("CONN").unwrap_or(0) + "CONN".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::DatabaseType::Oracle),
    )
    .expect("expected CREATE PROFILE statement bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE PROFILE app_profile LIMIT"),
        "CONNECT_TIME prefix must remain in CREATE PROFILE: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_keeps_oracle_create_user_password_option() {
    let sql = "CREATE USER app_user IDENTIFIED BY secret
  PROFILE app_profile
  PASSWORD EXPI
  ACCOUNT LOCK;";
    let cursor = sql.find("EXPI").unwrap_or(0) + "EXPI".len();

    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::DatabaseType::Oracle),
    )
    .expect("expected CREATE USER statement bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE USER app_user"),
        "PASSWORD option must remain in CREATE USER: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_recovers_incomplete_create_before_values_statement() {
    let sql = "CREATE OR REPLACE FUNCTION f RETURN NUMBER IS
BEGIN
  RETURN 1;
END
VALUES (42);
SELECT 2 FROM dual;";
    let cursor = sql.find("VALUES (42)").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected VALUES statement bounds after incomplete CREATE recovery");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("VALUES (42)"),
        "cursor on VALUES should resolve VALUES statement, got: {statement}"
    );
    assert!(
        !statement.contains("CREATE OR REPLACE FUNCTION"),
        "incomplete CREATE should not be merged into VALUES statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_recovers_incomplete_create_before_table_statement() {
    let sql = "CREATE OR REPLACE FUNCTION f RETURN NUMBER IS
BEGIN
  RETURN 1;
END
TABLE t_parser_recover;
SELECT 2 FROM dual;";
    let cursor = sql.find("TABLE t_parser_recover").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected TABLE statement bounds after incomplete CREATE recovery");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("TABLE t_parser_recover"),
        "cursor on TABLE should resolve TABLE statement, got: {statement}"
    );
    assert!(
        !statement.contains("CREATE OR REPLACE FUNCTION"),
        "incomplete CREATE should not be merged into TABLE statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_create_java_source_ignores_body_semicolon_until_slash() {
    let sql = r#"CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED "DemoClass" AS
public class DemoClass {
  public static String hello() {
    return "hello";
  }
}
/
SELECT 2 FROM dual;"#;
    let cursor = sql.find("return \"hello\"").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected JAVA SOURCE statement bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE OR REPLACE AND COMPILE JAVA SOURCE"),
        "JAVA SOURCE header should be included: {statement}"
    );
    assert!(
        statement.contains("return \"hello\";"),
        "JAVA body semicolon should remain inside one statement: {statement}"
    );
    assert!(
        !statement.contains("SELECT 2 FROM dual"),
        "slash-delimited trailing statement must not be merged: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mega_torture_resolves_trigger_after_package_body() {
    let sql = load_query_test_file("mega_torture.txt");
    let cursor = sql
        .find("CREATE OR REPLACE TRIGGER oqt_trg_test_bi")
        .unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(&sql, cursor)
        .expect("expected trigger statement bounds in mega_torture");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement
            .trim_start()
            .starts_with("CREATE OR REPLACE TRIGGER oqt_trg_test_bi"),
        "cursor on trigger should resolve trigger statement, got: {statement}"
    );
    assert!(
        !statement.contains("END oqt_mega_pkg;") && !statement.contains("SHOW ERRORS PACKAGE BODY"),
        "trigger statement bounds must not include package body tail: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mega_torture_resolves_execute_block_after_trigger() {
    let sql = load_query_test_file("mega_torture.txt");
    let cursor = sql.find("oqt_mega_pkg.seed(30)").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(&sql, cursor)
        .expect("expected execute block bounds in mega_torture");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("BEGIN"),
        "cursor on package execution block should resolve BEGIN block, got: {statement}"
    );
    assert!(
        statement.contains("oqt_mega_pkg.seed(30);"),
        "execution block should contain the SEED call: {statement}"
    );
    assert!(
        !statement.contains("CREATE OR REPLACE TRIGGER oqt_trg_test_bi"),
        "execution block must not include prior trigger statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mega_torture_resolves_summary_query_after_print() {
    let sql = load_query_test_file("mega_torture.txt");
    let cursor = sql.find("SELECT grp,").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(&sql, cursor)
        .expect("expected summary query bounds in mega_torture");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT grp,"),
        "cursor on summary query should resolve trailing SELECT, got: {statement}"
    );
    assert!(
        !statement.contains("PRINT v_rc") && !statement.contains("BEGIN\n    oqt_mega_pkg.open_rc"),
        "summary query must not include PRINT or prior BEGIN block: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_external_language_clause_without_external_suffix_with_slash_splits_following_select(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only_slash RETURN NUMBER
AS LANGUAGE C;
/
SELECT 2 FROM dual;"#;
    let cursor = sql.rfind("SELECT 2").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for trailing SELECT");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "cursor on trailing SELECT should resolve only SELECT statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_external_language_clause_without_external_suffix_splits_following_select_without_slash(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER
AS LANGUAGE C;
SELECT 3 FROM dual;"#;
    let cursor = sql.rfind("SELECT 3").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for trailing SELECT");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 3 FROM dual"),
        "cursor on trailing SELECT should resolve only SELECT statement: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_language_identifier_with_external_target_like_type_keeps_procedure_body(
) {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_shadow_c IS
  language c;
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let cursor = sql.find("NULL").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected procedure statement bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE OR REPLACE PROCEDURE proc_shadow_c IS"),
        "procedure header should stay in first statement bounds: {statement}"
    );
    assert!(
        statement.contains("language c;"),
        "LANGUAGE declaration should stay inside procedure body statement: {statement}"
    );
    assert!(
        statement.contains("END"),
        "procedure body should include END token: {statement}"
    );
    assert!(
        !statement.contains("SELECT 1 FROM dual"),
        "trailing SELECT must not be merged into procedure bounds: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_slash_after_end_without_semicolon_returns_previous_block() {
    let sql = "BEGIN\n  NULL;\nEND\n/\nSELECT 2 FROM dual;";
    let cursor = sql.find('/').unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected previous PL/SQL statement bounds on slash line");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("BEGIN"),
        "slash line should resolve to preceding block statement: {statement}"
    );
    assert!(
        statement.contains("END"),
        "preceding block statement should include END token: {statement}"
    );
    assert!(
        !statement.contains("SELECT 2 FROM dual"),
        "trailing SELECT must not leak into slash-line statement bounds: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_incomplete_create_recovers_before_rename_statement_head() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_recover AS\nBEGIN\n  NULL;\nEND\nRENAME t_parser_recover TO t_parser_recover_new;\nSELECT 2 FROM dual;";
    let cursor = sql.find("RENAME").unwrap_or(0);

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected recovered RENAME statement bounds after incomplete CREATE");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("RENAME t_parser_recover TO t_parser_recover_new"),
        "cursor on trailing RENAME should resolve only recovered statement: {statement}"
    );
    assert!(
        !statement.contains("CREATE OR REPLACE PROCEDURE"),
        "recovered statement must not include preceding CREATE text: {statement}"
    );
}

#[test]
fn test_normalize_sql_for_execute_trims_trailing_semicolon_for_select() {
    let normalized = QueryExecutor::normalize_sql_for_execute("  SELECT 1 FROM dual;   ");
    assert_eq!(normalized, "SELECT 1 FROM dual");
}

#[test]
fn test_normalize_sql_for_execute_keeps_plsql_block_semicolon() {
    let normalized = QueryExecutor::normalize_sql_for_execute("BEGIN NULL; END;  ");
    assert_eq!(normalized, "BEGIN NULL; END;");
}

#[test]
fn test_normalize_sql_for_execute_empty_input_stays_empty() {
    let normalized = QueryExecutor::normalize_sql_for_execute("  ;  \n\t ");
    assert!(normalized.is_empty());
}

#[test]
fn test_normalize_sql_for_execute_collapses_extra_trailing_semicolons_for_plsql() {
    let normalized = QueryExecutor::normalize_sql_for_execute("BEGIN NULL; END;;;   ");
    assert_eq!(normalized, "BEGIN NULL; END;");
}

#[test]
fn test_normalize_sql_for_execute_comment_prefixed_plsql_keeps_single_terminator() {
    let normalized =
        QueryExecutor::normalize_sql_for_execute("/*x*/ DECLARE v NUMBER:=1; BEGIN NULL; END;;; ");
    assert_eq!(normalized, "/*x*/ DECLARE v NUMBER:=1; BEGIN NULL; END;");
}

#[test]
fn test_normalize_sql_for_execute_removes_sqlplus_slash_for_single_statement() {
    let normalized = QueryExecutor::normalize_sql_for_execute("SELECT 1 FROM dual\n/\n");
    assert_eq!(normalized, "SELECT 1 FROM dual");
}

#[test]
fn test_normalize_sql_for_execute_removes_sqlplus_slash_for_plsql_block() {
    let normalized = QueryExecutor::normalize_sql_for_execute("BEGIN NULL; END;\n/\n");
    assert_eq!(normalized, "BEGIN NULL; END;");
}

#[test]
fn test_normalize_sql_for_execute_keeps_create_procedure_end_terminator() {
    let normalized = QueryExecutor::normalize_sql_for_execute(
        "CREATE OR REPLACE PROCEDURE p IS BEGIN NULL; END;;;   ",
    );
    assert_eq!(
        normalized,
        "CREATE OR REPLACE PROCEDURE p IS BEGIN NULL; END;"
    );
}

#[test]
fn test_is_plain_rollback_rejects_savepoint_clause() {
    assert!(!QueryExecutor::is_plain_rollback(
        "ROLLBACK TO SAVEPOINT before_update"
    ));
}

#[test]
fn test_is_plain_commit_rejects_non_plain_commit_variants() {
    assert!(QueryExecutor::is_plain_commit("COMMIT"));
    assert!(QueryExecutor::is_plain_commit("COMMIT WORK"));
    assert!(!QueryExecutor::is_plain_commit("COMMIT FORCE 'txn-id'"));
}

#[test]
fn test_split_script_items_ignores_trailing_remark_comment_line() {
    let sql = "SELECT 1 FROM dual
REMARK trailing note";
    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(statements.len(), 1);
    assert_eq!(statements[0].trim(), "SELECT 1 FROM dual");
}

#[test]
fn test_normalize_sql_for_execute_keeps_division_operator() {
    let normalized = QueryExecutor::normalize_sql_for_execute("SELECT 10/2 FROM dual");
    assert_eq!(normalized, "SELECT 10/2 FROM dual");
}

#[test]
fn test_simple_select() {
    let sql = "SELECT 1 FROM DUAL;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1);
    assert!(stmts[0].contains("SELECT 1 FROM DUAL"));
}

#[test]
fn test_multiple_selects() {
    let sql = "SELECT 1 FROM DUAL;\nSELECT 2 FROM DUAL;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2);
}

#[test]
fn test_is_select_statement_values_is_select() {
    let sql = "VALUES (1, 'A')";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "top-level VALUES should be treated as SELECT-like query"
    );
}

#[test]
fn test_is_select_statement_table_is_select() {
    let sql = "TABLE employees";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "top-level TABLE should be treated as SELECT-like query"
    );
}

#[test]
fn test_is_select_statement_with_clause_values_is_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) VALUES ((SELECT id FROM t))";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "WITH ... VALUES should be treated as SELECT-like query"
    );
}

#[test]
fn test_is_select_statement_with_clause_table_is_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) TABLE t";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "WITH ... TABLE should be treated as SELECT-like query"
    );
}

#[test]
fn test_is_select_statement_with_clause_insert_is_not_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) INSERT INTO t2(id) SELECT id FROM t";
    assert!(
        !QueryExecutor::is_select_statement(sql),
        "WITH ... INSERT should not be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_clause_update_is_not_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) UPDATE t2 SET id = (SELECT id FROM t)";
    assert!(
        !QueryExecutor::is_select_statement(sql),
        "WITH ... UPDATE should not be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_clause_delete_is_not_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) DELETE FROM t2 WHERE id IN (SELECT id FROM t)";
    assert!(
        !QueryExecutor::is_select_statement(sql),
        "WITH ... DELETE should not be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_clause_select_is_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) SELECT id FROM t";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "WITH ... SELECT should be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_parenthesized_main_select_is_select() {
    let sql = "WITH t AS (SELECT 1 AS id FROM dual) (SELECT id FROM t)";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "WITH ... (SELECT ...) should be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_parenthesized_set_query_is_select() {
    let sql = "(SELECT id FROM left_source) INTERSECT (SELECT id FROM right_source) MINUS (SELECT id FROM excluded_source) ORDER BY id";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "a parenthesized set query should use the query execution path"
    );
}

#[test]
fn test_is_select_statement_with_clause_merge_is_not_select() {
    let sql = "WITH src AS (SELECT 1 AS id FROM dual) MERGE INTO t2 d USING src s ON (d.id = s.id) WHEN MATCHED THEN UPDATE SET d.id = s.id";
    assert!(
        !QueryExecutor::is_select_statement(sql),
        "WITH ... MERGE should not be treated as SELECT"
    );
}

#[test]
fn test_is_select_statement_with_clause_ignores_comments_and_q_quotes() {
    let sql = "WITH t AS (SELECT q'[INSERT INTO t2 VALUES(1)]' AS txt FROM dual)
/* leading DML keyword in comment: DELETE */
SELECT txt FROM t";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "WITH ... SELECT should remain SELECT even with DML-like text in comments/strings"
    );
}

#[test]
fn test_is_select_statement_with_clause_invalid_q_quote_delimiter_does_not_hide_select() {
    let sql = "WITH t AS (SELECT q' INSERT INTO t2 VALUES(1)' AS txt FROM dual)\nSELECT txt FROM t";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "invalid q-quote delimiter should not leave parser stuck in quote mode"
    );
}

#[test]
fn test_is_select_statement_with_clause_invalid_nq_quote_delimiter_does_not_hide_select() {
    let sql = "WITH t AS (SELECT nq' UPDATE t2 SET c = 1' AS txt FROM dual)\nSELECT txt FROM t";
    assert!(
        QueryExecutor::is_select_statement(sql),
        "invalid nq-quote delimiter should not leave parser stuck in quote mode"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_injects_for_simple_single_table_select() {
    let sql = "SELECT ENAME, JOB FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "SELECT EMP.ROWID, ENAME, JOB FROM EMP");
}

#[test]
fn test_maybe_inject_rowid_for_editing_handles_vector_distance_operators() {
    let sql = "SELECT target_id, FROM_VECTOR (embedding + VECTOR ('[1, 1, 1]', 3, FLOAT32)) AS vector_sum,\n\
    FROM_VECTOR (embedding * VECTOR ('[2, 3, 4]', 3, FLOAT32)) AS vector_product,\n\
    ROUND (embedding <-> VECTOR ('[1, 1, 1]', 3, FLOAT32), 6) AS l2_distance,\n\
    ROUND (embedding <=> VECTOR ('[1, 1, 1]', 3, FLOAT32), 6) AS cosine_distance,\n\
    ROUND (embedding <#> VECTOR ('[1, 1, 1]', 3, FLOAT32), 6) AS neg_dot_product\n\
FROM sq_hard_w4_target\n\
ORDER BY target_id";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        format!(
            "SELECT sq_hard_w4_target.ROWID, {}",
            sql.strip_prefix("SELECT ").expect("SELECT prefix")
        )
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_dual() {
    let sql = "SELECT 1 AS one FROM dual";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);

    let schema_sql = r#"SELECT 1 AS one FROM "SYS"."DUAL""#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(schema_sql);
    assert_eq!(rewritten, schema_sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_keeps_existing_rowid() {
    let sql = "SELECT ROWID, ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_flashback_and_temporal_queries() {
    for sql in [
        "SELECT ENAME FROM EMP AS OF SCN 123",
        "SELECT ENAME FROM EMP AS OF TIMESTAMP (SYSTIMESTAMP - INTERVAL '5' SECOND)",
        "SELECT ENAME FROM EMP AS OF PERIOD FOR validity DATE '2026-02-01'",
        "SELECT ENAME FROM EMP VERSIONS BETWEEN SCN MINVALUE AND MAXVALUE",
        "SELECT ENAME FROM EMP VERSIONS PERIOD FOR validity BETWEEN DATE '2026-01-01' AND DATE '2026-02-01'",
    ] {
        assert_eq!(
            QueryExecutor::maybe_inject_rowid_for_editing(sql),
            sql,
            "flashback and temporal snapshots must not become editable"
        );
        assert!(
            !QueryExecutor::is_rowid_edit_eligible_query(sql),
            "flashback and temporal snapshots must not be ROWID-edit eligible"
        );
    }
}

#[test]
fn test_maybe_inject_rowid_for_editing_injects_for_join_query() {
    let sql = "SELECT e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_injects_for_multi_table_from_comma_join() {
    let sql = "SELECT ENAME FROM EMP e, DEPT d WHERE e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_where_in_comma_values() {
    let sql = "SELECT ENAME FROM EMP WHERE DEPTNO IN (10, 20)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT EMP.ROWID, ENAME FROM EMP WHERE DEPTNO IN (10, 20)"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_injects_for_with_clause_referencing_base_table() {
    let sql = "WITH dept_avg AS (SELECT DEPTNO, AVG(SAL) avg_sal FROM EMP GROUP BY DEPTNO) SELECT ENAME, SAL FROM EMP e JOIN dept_avg d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_with_clause_only_cte_ref() {
    // When the main SELECT FROM only references a CTE (not a base table), skip.
    let sql = "WITH e AS (SELECT ENAME FROM EMP) SELECT ENAME FROM e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_distinct() {
    let sql = "SELECT DISTINCT ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_mysql_distinctrow() {
    let sql = "SELECT DISTINCTROW ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
    assert!(!QueryExecutor::is_rowid_edit_eligible_query(sql));
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_distinct_with_comment_between_select_and_modifier() {
    let sql = "SELECT /* keep dedup */ DISTINCT ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_unique() {
    let sql = "SELECT UNIQUE ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_unique_with_newline_and_comment() {
    let sql = "SELECT\n-- preserve unique semantics\nUNIQUE ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_preserves_leading_hint_position() {
    let sql = "SELECT /*+ INDEX(emp emp_idx1) */ ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT /*+ INDEX(emp emp_idx1) */ EMP.ROWID, ENAME FROM EMP"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_preserves_hint_before_all_modifier() {
    let sql = "SELECT /*+ FULL(emp) */ ALL ENAME FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT /*+ FULL(emp) */ ALL EMP.ROWID, ENAME FROM EMP"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_uses_alias_when_present() {
    let sql = "SELECT ENAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "SELECT e.ROWID, ENAME FROM EMP e");
}

#[test]
fn test_maybe_inject_rowid_for_editing_qualifies_leading_wildcard_with_alias() {
    let sql = "SELECT * FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "SELECT e.ROWID, e.* FROM EMP e");
}

#[test]
fn test_maybe_inject_rowid_for_editing_qualifies_leading_wildcard_with_table_name() {
    let sql = "select * from help;";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "select help.ROWID, help.* from help;");
}

#[test]
fn test_rowid_safe_execution_sql_rewrites_auto_injected_projection() {
    let source_sql = "SELECT ENAME FROM EMP e";
    let injected = QueryExecutor::maybe_inject_rowid_for_editing(source_sql);
    let safe_sql = QueryExecutor::rowid_safe_execution_sql(source_sql, &injected);
    assert_eq!(
        safe_sql,
        "SELECT ROWIDTOCHAR(e.ROWID) AS SQ_INTERNAL_ROWID, ENAME FROM EMP e"
    );
}

#[test]
fn test_rowid_safe_execution_sql_rewrites_user_rowid_query() {
    let source_sql = "SELECT ROWID, ENAME FROM EMP";
    let safe_sql = QueryExecutor::rowid_safe_execution_sql(source_sql, source_sql);
    assert_eq!(
        safe_sql,
        "SELECT ROWIDTOCHAR(ROWID) AS SQ_INTERNAL_ROWID, ENAME FROM EMP"
    );
}

#[test]
fn test_rowid_safe_execution_sql_keeps_non_rowid_internal_alias_projection() {
    let source_sql = "SELECT ENAME AS SQ_INTERNAL_ROWID, ENAME FROM EMP";
    let safe_sql = QueryExecutor::rowid_safe_execution_sql(source_sql, source_sql);
    assert_eq!(safe_sql, source_sql);
}

#[test]
fn test_rowid_safe_execution_sql_handles_order_by_wildcard_query() {
    let source_sql = "SELECT * FROM oqt_run_log ORDER BY run_ts DESC";
    let injected = QueryExecutor::maybe_inject_rowid_for_editing(source_sql);
    let safe_sql = QueryExecutor::rowid_safe_execution_sql(source_sql, &injected);
    assert_eq!(
        safe_sql,
        "SELECT ROWIDTOCHAR(oqt_run_log.ROWID) AS SQ_INTERNAL_ROWID, oqt_run_log.* FROM oqt_run_log ORDER BY run_ts DESC"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_table_collection_expression() {
    let sql = "SELECT * FROM TABLE(oqt_demo_pkg.func_pipe_rows(7000)) ORDER BY sal";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_schema_qualified_relation_invocation() {
    let sql = "SELECT * FROM oqt_demo_pkg.func_pipe_rows(7000) f";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_lateral_relation_invocation() {
    let sql = "SELECT * FROM LATERAL oqt_demo_pkg.func_pipe_rows(7000) f";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

// --- Multi-table / JOIN / CTE / subquery test cases ---

#[test]
fn test_maybe_inject_rowid_for_editing_left_join() {
    let sql = "SELECT e.ENAME, d.DNAME FROM EMP e LEFT JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_right_join() {
    let sql = "SELECT e.ENAME, d.DNAME FROM EMP e RIGHT JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_full_outer_join() {
    let sql = "SELECT e.ENAME, d.DNAME FROM EMP e FULL OUTER JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_cross_join() {
    let sql = "SELECT e.ENAME, d.DNAME FROM EMP e CROSS JOIN DEPT d";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_natural_join() {
    let sql = "SELECT ENAME, DNAME FROM EMP e NATURAL JOIN DEPT d";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_multiple_joins() {
    let sql = "SELECT e.ENAME, d.DNAME, s.GRADE FROM EMP e JOIN DEPT d ON e.DEPTNO = d.DEPTNO JOIN SALGRADE s ON e.SAL BETWEEN s.LOSAL AND s.HISAL";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_without_alias() {
    let sql = "SELECT ENAME, DNAME FROM EMP JOIN DEPT ON EMP.DEPTNO = DEPT.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_with_schema_prefix() {
    let sql = "SELECT e.ENAME, d.DNAME FROM SCOTT.EMP e JOIN SCOTT.DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_with_quoted_alias() {
    let sql = r#"SELECT "e".ENAME FROM EMP "e" JOIN DEPT "d" ON "e".DEPTNO = "d".DEPTNO"#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_clause_and_join_to_base_table() {
    let sql = "WITH recent AS (SELECT DEPTNO FROM DEPT WHERE LOC = 'DALLAS') SELECT e.ENAME FROM EMP e JOIN recent r ON e.DEPTNO = r.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_clause_single_base_table() {
    let sql = "WITH dept_info AS (SELECT DEPTNO, DNAME FROM DEPT) SELECT e.ENAME, d.DNAME FROM EMP e, dept_info d WHERE e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_multiple_ctes() {
    let sql = "WITH cte1 AS (SELECT 1 AS x FROM DUAL), cte2 AS (SELECT 2 AS y FROM DUAL) SELECT ENAME FROM EMP e WHERE e.DEPTNO = 10";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "WITH cte1 AS (SELECT 1 AS x FROM DUAL), cte2 AS (SELECT 2 AS y FROM DUAL) SELECT e.ROWID, ENAME FROM EMP e WHERE e.DEPTNO = 10"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_parenthesized_main_select() {
    let sql = "WITH t AS (SELECT EMPNO, ENAME FROM EMP) (SELECT ENAME FROM EMP)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_union() {
    let sql = "SELECT ENAME FROM EMP UNION SELECT DNAME FROM DEPT";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_union_all() {
    let sql = "SELECT ENAME FROM EMP UNION ALL SELECT DNAME FROM DEPT";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_intersect() {
    let sql = "SELECT DEPTNO FROM EMP INTERSECT SELECT DEPTNO FROM DEPT";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_minus() {
    let sql = "SELECT DEPTNO FROM EMP MINUS SELECT DEPTNO FROM DEPT";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_group_by() {
    let sql = "SELECT DEPTNO, COUNT(*) FROM EMP GROUP BY DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_nested_aggregate_wrappers() {
    for sql in [
        r#"SELECT XMLSERIALIZE(
                 CONTENT XMLELEMENT("metrics",
                   XMLAGG(
                     XMLELEMENT("m", XMLFOREST(m.node_id AS "node"))
                     ORDER BY m.metric_id))
                 AS CLOB) AS xml_report
            FROM sq_hard_metric m"#,
        "SELECT ROUND(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY sal), 2) FROM emp",
        "SELECT XMLELEMENT(\"stats\", SYS_XMLAGG(XMLELEMENT(\"v\", sal))) FROM emp",
    ] {
        let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
        assert_eq!(rewritten, sql);
    }
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_mysql_family_aggregates() {
    for function in [
        "BIT_AND",
        "BIT_OR",
        "BIT_XOR",
        "GROUP_CONCAT",
        "STD",
        "ST_COLLECT",
    ] {
        let sql = format!("SELECT {function}(value_col) FROM metrics");
        assert_eq!(QueryExecutor::maybe_inject_rowid_for_editing(&sql), sql);
        assert!(!QueryExecutor::is_rowid_edit_eligible_query(&sql));
    }
}

#[test]
fn test_maybe_inject_rowid_for_editing_ignores_aggregate_in_scalar_subquery() {
    let sql = "SELECT ENAME, COALESCE((SELECT AVG(SAL) FROM EMP), 0) AS AVG_SAL FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME, COALESCE((SELECT AVG(SAL) FROM EMP), 0) AS AVG_SAL FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_connect_by() {
    let sql =
        "SELECT EMPNO, MGR, LEVEL FROM EMP CONNECT BY PRIOR EMPNO = MGR START WITH MGR IS NULL";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_ignores_q_quote_fake_from_keyword() {
    let sql = "SELECT q'[FROM not_a_clause]' AS txt, ENAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, q'[FROM not_a_clause]' AS txt, ENAME FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_ignores_nq_quote_fake_join_keyword() {
    let sql = "SELECT nq'[JOIN fake table]' AS txt, ENAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, nq'[JOIN fake table]' AS txt, ENAME FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_ignores_q_quote_fake_over_keyword() {
    let sql = "SELECT q'[OVER (PARTITION BY DEPTNO)]' AS expr, ENAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, q'[OVER (PARTITION BY DEPTNO)]' AS expr, ENAME FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_analytic_over_clause() {
    let sql = "SELECT ENAME, ROW_NUMBER() OVER (PARTITION BY DEPTNO ORDER BY SAL DESC) RN FROM EMP";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_pivot_clause() {
    let sql =
        "SELECT * FROM (SELECT JOB, DEPTNO, SAL FROM EMP) PIVOT (SUM(SAL) FOR DEPTNO IN (10, 20))";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_model_clause() {
    let sql = "SELECT ENAME, DEPTNO, SAL FROM EMP MODEL RETURN UPDATED ROWS DIMENSION BY (DEPTNO) MEASURES (SAL) RULES (SAL[10] = 0)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_match_recognize_clause() {
    let sql = r#"SELECT *
FROM oqt_t_emp
MATCH_RECOGNIZE (
  PARTITION BY deptno
  ORDER BY hiredate, empno
  MEASURES
    FIRST(ename) AS start_name,
    LAST(ename)  AS end_name,
    COUNT(*)     AS run_len
  ONE ROW PER MATCH
  PATTERN (a b+)
  DEFINE
    b AS b.sal > PREV(b.sal)
)"#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_subquery_in_where() {
    let sql =
        "SELECT ENAME FROM EMP e WHERE DEPTNO IN (SELECT DEPTNO FROM DEPT WHERE LOC = 'DALLAS')";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME FROM EMP e WHERE DEPTNO IN (SELECT DEPTNO FROM DEPT WHERE LOC = 'DALLAS')"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_correlated_subquery() {
    let sql = "SELECT ENAME, SAL FROM EMP e WHERE SAL > (SELECT AVG(SAL) FROM EMP WHERE DEPTNO = e.DEPTNO)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME, SAL FROM EMP e WHERE SAL > (SELECT AVG(SAL) FROM EMP WHERE DEPTNO = e.DEPTNO)"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_exists_subquery() {
    let sql =
        "SELECT ENAME FROM EMP e WHERE EXISTS (SELECT 1 FROM DEPT d WHERE d.DEPTNO = e.DEPTNO)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME FROM EMP e WHERE EXISTS (SELECT 1 FROM DEPT d WHERE d.DEPTNO = e.DEPTNO)"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_scalar_subquery_in_select() {
    let sql = "SELECT ENAME, (SELECT DNAME FROM DEPT d WHERE d.DEPTNO = e.DEPTNO) AS DEPT_NAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME, (SELECT DNAME FROM DEPT d WHERE d.DEPTNO = e.DEPTNO) AS DEPT_NAME FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_skips_from_subquery_as_source() {
    // When FROM clause starts with a subquery, ROWID is not available
    let sql = "SELECT x.ENAME FROM (SELECT ENAME FROM EMP) x";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_with_as_alias() {
    let sql = "SELECT e.ENAME FROM EMP AS e JOIN DEPT AS d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_with_where_clause() {
    let sql =
        "SELECT e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON e.DEPTNO = d.DEPTNO WHERE e.SAL > 1000";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_join_wildcard_qualifies_first_table() {
    let sql = "SELECT * FROM EMP e JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_keeps_existing_qualified_rowid() {
    let sql = "SELECT e.ROWID, e.ENAME FROM EMP e JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_recursive_cte() {
    // Recursive CTE whose main SELECT reads from the CTE should not get ROWID.
    let sql = "WITH RECURSIVE mgr_chain AS (SELECT EMPNO, ENAME, MGR FROM EMP WHERE EMPNO = 7369 UNION ALL SELECT e.EMPNO, e.ENAME, e.MGR FROM EMP e JOIN mgr_chain m ON e.EMPNO = m.MGR) SELECT ENAME FROM mgr_chain";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, sql);
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_quoted_table_name() {
    let sql = r#"SELECT "Employee Name" FROM "My Table" t"#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        r#"SELECT t.ROWID, "Employee Name" FROM "My Table" t"#
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_with_schema_no_alias() {
    let sql = "SELECT ENAME FROM HR.EMPLOYEES";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT HR.EMPLOYEES.ROWID, ENAME FROM HR.EMPLOYEES"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_allows_having_without_group_by_keyword() {
    // HAVING without GROUP BY is unusual but valid in some dialects
    let sql = "SELECT ENAME FROM EMP e WHERE SAL > 1000 ORDER BY ENAME";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, ENAME FROM EMP e WHERE SAL > 1000 ORDER BY ENAME"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_group_by_in_subquery_not_top_level() {
    // GROUP BY inside subquery should NOT block injection (only top-level GROUP matters)
    let sql = "SELECT e.ENAME FROM EMP e WHERE e.DEPTNO IN (SELECT DEPTNO FROM EMP GROUP BY DEPTNO HAVING COUNT(*) > 3)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, e.ENAME FROM EMP e WHERE e.DEPTNO IN (SELECT DEPTNO FROM EMP GROUP BY DEPTNO HAVING COUNT(*) > 3)"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_union_in_subquery_not_top_level() {
    // UNION inside a subquery should NOT block injection
    let sql = "SELECT e.ENAME FROM EMP e WHERE e.DEPTNO IN (SELECT DEPTNO FROM DEPT UNION SELECT DEPTNO FROM EMP)";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, e.ENAME FROM EMP e WHERE e.DEPTNO IN (SELECT DEPTNO FROM DEPT UNION SELECT DEPTNO FROM EMP)"
    );
}

// --- Bug regression tests: identifier boundary / quoted alias ---

#[test]
fn test_maybe_inject_rowid_for_editing_start_date_column_not_blocked() {
    // Bug fix: START_DATE column must NOT be mistaken for "START WITH" hierarchical keyword
    let sql = "SELECT e.ENAME, e.START_DATE FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(
        rewritten,
        "SELECT e.ROWID, e.ENAME, e.START_DATE FROM EMP e"
    );
}

#[test]
fn test_maybe_inject_rowid_for_editing_group_id_column_not_blocked() {
    // Bug fix: GROUP_ID column must NOT be mistaken for "GROUP BY" keyword
    let sql = "SELECT GROUP_ID, ENAME FROM EMP e";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "SELECT e.ROWID, GROUP_ID, ENAME FROM EMP e");
}

#[test]
fn test_maybe_inject_rowid_for_editing_connect_string_column_not_blocked() {
    // CONNECT_STRING column must NOT be mistaken for "CONNECT BY"
    let sql = "SELECT CONNECT_STRING FROM CONFIG c";
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, "SELECT c.ROWID, CONNECT_STRING FROM CONFIG c");
}

#[test]
fn test_maybe_inject_rowid_for_editing_quoted_keyword_alias() {
    // Bug fix: quoted alias matching a keyword (e.g. "WHERE") must NOT be rejected
    let sql = r#"SELECT ENAME FROM EMP "where""#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, r#"SELECT "where".ROWID, ENAME FROM EMP "where""#);
}

#[test]
fn test_maybe_inject_rowid_for_editing_quoted_join_alias() {
    // Quoted alias "join" must be accepted as an alias
    let sql = r#"SELECT ENAME FROM EMP "join""#;
    let rewritten = QueryExecutor::maybe_inject_rowid_for_editing(sql);
    assert_eq!(rewritten, r#"SELECT "join".ROWID, ENAME FROM EMP "join""#);
}

#[test]
fn test_maybe_inject_rowid_for_editing_keyword_like_aliases_from_test13() {
    let simple_alias_sql = "SELECT count.a, count.b FROM qt_kw_base count ORDER BY count.a";
    let simple_rewritten = QueryExecutor::maybe_inject_rowid_for_editing(simple_alias_sql);
    assert_eq!(
        simple_rewritten,
        "SELECT count.ROWID, count.a, count.b FROM qt_kw_base count ORDER BY count.a"
    );

    let with_keyword_cte_sql = "WITH level AS (SELECT a, b FROM qt_kw_base) SELECT level.a, level.b FROM level ORDER BY level.a";
    assert!(
        QueryExecutor::is_select_statement(with_keyword_cte_sql),
        "WITH level CTE should be classified as SELECT"
    );
}

#[test]
fn test_retryable_rowid_injection_error_detects_non_key_preserved_table() {
    let message =
        "ORA-01445: cannot select ROWID from, or sample, a join view without a key-preserved table";
    assert!(QueryExecutor::is_retryable_rowid_injection_error(message));
}

#[test]
fn test_retryable_rowid_injection_error_detects_rowid_illegal_here() {
    let message =
        "ORA-01446: cannot select ROWID from, or sample, a view with DISTINCT, GROUP BY, etc.";
    assert!(QueryExecutor::is_retryable_rowid_injection_error(message));
}

#[test]
fn test_retryable_rowid_injection_error_detects_invalid_identifier_rowid() {
    let message = "ORA-00904: \"ROWID\": invalid identifier";
    assert!(QueryExecutor::is_retryable_rowid_injection_error(message));
}

#[test]
fn test_retryable_rowid_injection_error_detects_injected_grouping_errors() {
    for message in [
        "ORA-00937: not a single-group group function",
        "ORA-00979: not a GROUP BY expression",
    ] {
        assert!(QueryExecutor::is_retryable_rowid_injection_error(message));
    }
}

#[test]
fn test_retryable_rowid_injection_error_ignores_other_oracle_errors() {
    let message = "ORA-00942: table or view does not exist";
    assert!(!QueryExecutor::is_retryable_rowid_injection_error(message));
}

#[test]
fn test_is_plain_commit_allows_only_commit_variants() {
    assert!(QueryExecutor::is_plain_commit("COMMIT"));
    assert!(QueryExecutor::is_plain_commit("commit work;"));
    assert!(QueryExecutor::is_plain_commit("# leading comment\nCOMMIT"));
    assert!(QueryExecutor::is_plain_commit(
        "REM leading comment\nCOMMIT"
    ));
    assert!(QueryExecutor::is_plain_commit("COMMIT /* trailing */"));
    assert!(QueryExecutor::is_plain_commit(
        "COMMIT -- trailing comment\n"
    ));
    assert!(!QueryExecutor::is_plain_commit("COMMIT FORCE '1.2.3'"));
    assert!(!QueryExecutor::is_plain_commit("COMMIT COMMENT 'done'"));
    assert!(!QueryExecutor::is_plain_commit(
        "COMMIT -- line comment hides only this line\nAND CHAIN"
    ));
    assert!(!QueryExecutor::is_plain_commit(
        "COMMIT /* block comment */ AND CHAIN"
    ));
    assert!(!QueryExecutor::is_plain_commit(
        "COMMIT 'not a valid option'"
    ));
    assert!(!QueryExecutor::is_plain_commit("COMMIT /* unterminated"));
}

#[test]
fn test_is_plain_rollback_allows_only_rollback_variants() {
    assert!(QueryExecutor::is_plain_rollback("ROLLBACK"));
    assert!(QueryExecutor::is_plain_rollback("rollback work;"));
    assert!(QueryExecutor::is_plain_rollback(
        "# leading comment\nROLLBACK"
    ));
    assert!(QueryExecutor::is_plain_rollback(
        "REM leading comment\nROLLBACK"
    ));
    assert!(QueryExecutor::is_plain_rollback("ROLLBACK /* trailing */"));
    assert!(QueryExecutor::is_plain_rollback(
        "ROLLBACK -- trailing comment\n"
    ));
    assert!(!QueryExecutor::is_plain_rollback("ROLLBACK TO sp1"));
    assert!(!QueryExecutor::is_plain_rollback("ROLLBACK FORCE '1.2.3'"));
    assert!(!QueryExecutor::is_plain_rollback(
        "ROLLBACK -- line comment hides only this line\nAND CHAIN"
    ));
    assert!(!QueryExecutor::is_plain_rollback(
        "ROLLBACK /* block comment */ AND CHAIN"
    ));
    assert!(!QueryExecutor::is_plain_rollback(
        "ROLLBACK 'not a valid option'"
    ));
    assert!(!QueryExecutor::is_plain_rollback(
        "ROLLBACK /* unterminated"
    ));
}

#[test]
fn test_double_semicolon() {
    let sql = "SELECT 1 FROM DUAL;;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(
        !stmts[0].ends_with(";;"),
        "Should not end with ;;: {}",
        stmts[0]
    );
}

#[test]
fn test_anonymous_block() {
    let sql = "DECLARE x NUMBER; BEGIN x := 1; END;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_procedure_simple() {
    let sql = "CREATE PROCEDURE test_proc AS BEGIN NULL; END;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(stmts[0].contains("CREATE PROCEDURE"));
    assert!(stmts[0].contains("END"));
}

#[test]
fn test_create_procedure_with_declare() {
    let sql = r#"CREATE PROCEDURE test_proc AS
DECLARE
  v_num NUMBER;
BEGIN
  v_num := 1;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_or_replace_procedure() {
    let sql = r#"CREATE OR REPLACE PROCEDURE test_proc IS
BEGIN
  DBMS_OUTPUT.PUT_LINE('Hello');
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_or_replace_force_procedure() {
    let sql = r#"CREATE OR REPLACE FORCE PROCEDURE test_proc IS
BEGIN
  DBMS_OUTPUT.PUT_LINE('Hello');
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(stmts[0].contains("CREATE OR REPLACE FORCE PROCEDURE"));
}

#[test]
fn test_normalize_exec_call_handles_leading_comments() {
    let sql = "-- run proc\nEXEC test_proc(:v1);";
    let normalized = QueryExecutor::normalize_exec_call(sql);
    assert_eq!(normalized.as_deref(), Some("BEGIN test_proc(:v1); END;"));
}

#[test]
fn test_normalize_exec_call_handles_leading_whitespace() {
    let sql = "  \n\tEXEC test_proc(:v1);";
    let normalized = QueryExecutor::normalize_exec_call(sql);
    assert_eq!(normalized.as_deref(), Some("BEGIN test_proc(:v1); END;"));
}

#[test]
fn test_check_named_positional_mix_handles_leading_whitespace() {
    let sql = "\n  EXEC test_proc(p_id => 1, 2);";
    assert!(QueryExecutor::check_named_positional_mix(sql).is_err());
}

#[test]
fn test_check_named_positional_mix_ignores_line_comment_arrow() {
    let sql = "EXEC test_proc(1, -- p_id => 1\n 2);";
    assert!(QueryExecutor::check_named_positional_mix(sql).is_ok());
}

#[test]
fn test_check_named_positional_mix_ignores_block_comment_arrow() {
    let sql = "EXEC test_proc(1, /* p_id => 1 */ 2);";
    assert!(QueryExecutor::check_named_positional_mix(sql).is_ok());
}

#[test]
fn test_check_named_positional_mix_call_is_not_validated_as_exec() {
    let sql = "CALL test_proc(p_id => 1, 2)";
    assert!(QueryExecutor::check_named_positional_mix(sql).is_ok());
}

#[test]
fn test_create_external_function_as_non_plsql_block_followed_by_select() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_fn(
  p_num NUMBER
) RETURN NUMBER
IS
EXTERNAL
  LANGUAGE C
  NAME 'ExtNativeFunction';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "External function declaration should split before following SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].contains("CREATE OR REPLACE FUNCTION"));
    assert!(stmts[1].contains("SELECT 1 FROM"));
}

#[test]
fn test_package_body_nested_external_procedure_followed_by_select_splits() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_ext AS
  PROCEDURE ext_proc IS
    EXTERNAL NAME "ext_proc" LANGUAGE C;
END pkg_ext;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "Package body with nested EXTERNAL procedure should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE PACKAGE BODY pkg_ext AS"),
        "first statement should keep full package body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_type_body_nested_external_member_function_followed_by_select_splits() {
    let sql = r#"CREATE OR REPLACE TYPE BODY oqt_obj AS
  MEMBER FUNCTION ext_fn RETURN NUMBER
  AS EXTERNAL
  NAME 'ExtFn'
  LANGUAGE C;
END oqt_obj;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "TYPE BODY with EXTERNAL member function should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE BODY oqt_obj AS"),
        "first statement should keep full TYPE BODY: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("MEMBER FUNCTION ext_fn RETURN NUMBER"),
        "TYPE BODY should include external member function: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 1 FROM dual"),
        "trailing SELECT should remain separate statement"
    );
}

#[test]
fn test_type_body_nested_external_member_procedure_followed_by_select_splits() {
    let sql = r#"CREATE OR REPLACE TYPE BODY oqt_obj AS
  MEMBER PROCEDURE ext_proc (p_id NUMBER)
  AS EXTERNAL
  NAME 'ExtProc'
  LANGUAGE C;
END oqt_obj;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "TYPE BODY with EXTERNAL member procedure should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE BODY oqt_obj AS"),
        "first statement should keep full TYPE BODY: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("MEMBER PROCEDURE ext_proc"),
        "TYPE BODY should include external member procedure: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 1 FROM dual"),
        "trailing SELECT should remain separate statement"
    );
}

#[test]
fn test_type_body_local_table_type_declaration_followed_by_select_splits() {
    let sql = r#"CREATE OR REPLACE TYPE BODY t_local_types AS
  MEMBER PROCEDURE p IS
    TYPE num_tab IS TABLE OF NUMBER;
  BEGIN
    NULL;
  END;
END t_local_types;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "TYPE BODY with local TABLE type declaration should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE BODY t_local_types AS"),
        "first statement should keep full TYPE BODY: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("TYPE num_tab IS TABLE OF NUMBER;"),
        "TYPE BODY should keep local TABLE type declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 1 FROM dual"),
        "trailing SELECT should remain separate statement"
    );
}

#[test]
fn test_split_format_items_type_body_local_ref_cursor_type_declaration_splits() {
    let sql = r#"CREATE OR REPLACE TYPE BODY t_local_ref AS
  MEMBER PROCEDURE p IS
    TYPE rc_t IS REF CURSOR;
  BEGIN
    NULL;
  END;
END t_local_ref;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep TYPE BODY with local REF CURSOR type as one statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE BODY t_local_ref AS"),
        "first formatted statement should keep full TYPE BODY: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("TYPE rc_t IS REF CURSOR;"),
        "first formatted statement should keep local REF CURSOR type declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "trailing SELECT should remain separate formatted statement: {}",
        stmts[1]
    );
}

#[test]
fn test_procedure_name_language_library_identifiers_do_not_trigger_external_split() {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_shadow IS
  name NUMBER := 1;
  language NUMBER := 2;
  library NUMBER := 3;
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "identifier tokens NAME/LANGUAGE/LIBRARY must not trigger EXTERNAL split, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow IS"),
        "first statement should keep full procedure body: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("name NUMBER := 1;")
            && stmts[0].contains("language NUMBER := 2;")
            && stmts[0].contains("library NUMBER := 3;")
            && stmts[0].contains("END"),
        "procedure body should preserve NAME/LANGUAGE/LIBRARY declarations: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_name_language_library_identifiers_do_not_trigger_external_split() {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_shadow IS
  name NUMBER := 1;
  language NUMBER := 2;
  library NUMBER := 3;
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must keep NAME/LANGUAGE/LIBRARY identifiers inside routine body: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow IS"));
    assert!(stmts[0].contains("name NUMBER := 1;"));
    assert!(stmts[0].contains("language NUMBER := 2;"));
    assert!(stmts[0].contains("library NUMBER := 3;"));
    assert!(stmts[0].contains("END"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_procedure_language_identifier_with_language_targets_do_not_trigger_external_split() {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_shadow_targets IS
  language c;
  language java;
  language javascript;
  language python;
  marker NUMBER := 1;
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE + external-target-like datatype declarations must not trigger EXTERNAL split, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow_targets IS"));
    assert!(stmts[0].contains("language c;"));
    assert!(stmts[0].contains("language java;"));
    assert!(stmts[0].contains("language javascript;"));
    assert!(stmts[0].contains("language python;"));
    assert!(stmts[0].contains("marker NUMBER := 1;"));
    assert!(stmts[0].contains("END"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_language_identifier_with_language_targets_do_not_trigger_external_split()
{
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_shadow_targets IS
  language c;
  language java;
  language javascript;
  language python;
  marker NUMBER := 1;
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must keep LANGUAGE + external-target-like datatype declarations inside routine body: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow_targets IS"));
    assert!(stmts[0].contains("language c;"));
    assert!(stmts[0].contains("language java;"));
    assert!(stmts[0].contains("language javascript;"));
    assert!(stmts[0].contains("language python;"));
    assert!(stmts[0].contains("marker NUMBER := 1;"));
    assert!(stmts[0].contains("END"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_procedure_language_assignment_does_not_trigger_external_split() {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_assign IS
  language := 'C';
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE assignment must not trigger EXTERNAL split, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_assign IS"));
    assert!(stmts[0].contains("language := 'C';"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_language_assignment_does_not_trigger_external_split() {
    let sql = r#"CREATE OR REPLACE PROCEDURE proc_assign IS
  language := 'C';
BEGIN
  NULL;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must keep LANGUAGE assignment inside routine body: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PROCEDURE proc_assign IS"));
    assert!(stmts[0].contains("language := 'C';"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_clause_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER
AS LANGUAGE C NAME 'ext_lang_only';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER"),
        "first statement should keep external call spec function: {}",
        stmts[0]
    );
    assert!(stmts[0].contains("AS LANGUAGE C NAME 'ext_lang_only'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_clause_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER
AS LANGUAGE C NAME 'ext_lang_only';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE call spec function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C NAME 'ext_lang_only'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_clause_with_qquoted_target_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_qquoted RETURN NUMBER
AS LANGUAGE q'[C]' NAME 'ext_lang_qquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE q-quoted target call spec should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_qquoted RETURN NUMBER"),
        "first statement should keep q-quoted LANGUAGE call spec function: {}",
        stmts[0]
    );
    assert!(stmts[0].contains("AS LANGUAGE q'[C]' NAME 'ext_lang_qquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_name_identifier_without_quotes_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_ident RETURN NUMBER
AS LANGUAGE C NAME ext_lang_ident;
SELECT 7 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "external call spec with identifier NAME target should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].contains("NAME ext_lang_ident"));
    assert!(stmts[1].starts_with("SELECT 7 FROM dual"));
}

#[test]
fn test_split_format_items_external_function_name_identifier_without_quotes_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_ident RETURN NUMBER
AS LANGUAGE C NAME ext_lang_ident;
SELECT 7 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep identifier NAME call spec function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].contains("NAME ext_lang_ident"));
    assert!(stmts[1].starts_with("SELECT 7 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_clause_with_nqquoted_target_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_nqquoted RETURN NUMBER
AS LANGUAGE nq'[C]' NAME 'ext_lang_nqquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep nq-quoted LANGUAGE call spec function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_nqquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE nq'[C]' NAME 'ext_lang_nqquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_clause_without_external_suffix_still_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER
AS LANGUAGE C;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE target-only call spec should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_clause_without_external_suffix_with_slash_still_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only_slash RETURN NUMBER
AS LANGUAGE C;
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE target-only call spec with slash delimiter should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_only_slash RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_clause_without_semicolon_uses_slash_terminator() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only_slash_no_semicolon RETURN NUMBER
AS LANGUAGE C
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE target-only call spec without semicolon should still split at slash delimiter, got: {:?}",
        stmts
    );
    assert!(stmts[0]
        .starts_with("CREATE OR REPLACE FUNCTION ext_lang_only_slash_no_semicolon RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_external_clause_without_semicolon_uses_slash_terminator() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_external_only_slash_no_semicolon RETURN NUMBER
AS EXTERNAL
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "EXTERNAL-only call spec without semicolon should still split at slash delimiter, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with(
        "CREATE OR REPLACE FUNCTION ext_external_only_slash_no_semicolon RETURN NUMBER"
    ));
    assert!(stmts[0].contains("AS EXTERNAL"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_clause_without_semicolon_uses_slash_terminator() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_external_only_slash_no_semicolon RETURN NUMBER
AS EXTERNAL
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should split EXTERNAL-only call spec without semicolon at slash, got: {:?}",
        stmts
    );
    assert_eq!(
        slash_count, 1,
        "expected one slash terminator, got: {items:?}"
    );
    assert!(stmts[0].starts_with(
        "CREATE OR REPLACE FUNCTION ext_external_only_slash_no_semicolon RETURN NUMBER"
    ));
    assert!(stmts[0].contains("AS EXTERNAL"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_clause_without_semicolon_uses_slash_terminator() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only_slash_no_semicolon RETURN NUMBER
AS LANGUAGE C
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should split LANGUAGE target-only call spec without semicolon at slash, got: {:?}",
        stmts
    );
    assert_eq!(
        slash_count, 1,
        "expected one slash terminator, got: {items:?}"
    );
    assert!(stmts[0]
        .starts_with("CREATE OR REPLACE FUNCTION ext_lang_only_slash_no_semicolon RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_clause_without_external_suffix_with_slash_still_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_only_slash RETURN NUMBER
AS LANGUAGE C;
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE target-only call spec with slash and split trailing SELECT: {:?}",
        stmts
    );
    assert_eq!(
        slash_count, 1,
        "expected one slash terminator, got: {items:?}"
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_only_slash RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_external_clause_without_suffix_still_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_external_only RETURN NUMBER
AS EXTERNAL;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "EXTERNAL-only call spec should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_external_only RETURN NUMBER"));
    assert!(stmts[0].contains("AS EXTERNAL"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_parameters_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_params RETURN NUMBER
AS LANGUAGE C PARAMETERS (CONTEXT);
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE ... PARAMETERS call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_params RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C PARAMETERS (CONTEXT)"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_calling_standard_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_calling RETURN NUMBER
AS LANGUAGE C CALLING STANDARD;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE ... CALLING STANDARD without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_calling RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C CALLING STANDARD"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_calling_standard_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_calling RETURN NUMBER
AS LANGUAGE C CALLING STANDARD;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE ... CALLING STANDARD function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_calling RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C CALLING STANDARD"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_with_context_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_context RETURN NUMBER
AS LANGUAGE C WITH CONTEXT;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE ... WITH CONTEXT without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_context RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C WITH CONTEXT"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_with_context_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_context RETURN NUMBER
AS LANGUAGE C WITH CONTEXT;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE ... WITH CONTEXT function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_context RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C WITH CONTEXT"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_mle_module_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_language_mle_module RETURN NUMBER
AS LANGUAGE MLE MODULE ext_mle_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "AS LANGUAGE MLE MODULE call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_language_mle_module RETURN NUMBER")
    );
    assert!(stmts[0].contains("AS LANGUAGE MLE MODULE ext_mle_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_mle_module_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_language_mle_module RETURN NUMBER
AS LANGUAGE MLE MODULE ext_mle_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep AS LANGUAGE MLE MODULE function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_language_mle_module RETURN NUMBER")
    );
    assert!(stmts[0].contains("AS LANGUAGE MLE MODULE ext_mle_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_mle_signature_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_language_mle_signature RETURN NUMBER
AS LANGUAGE MLE SIGNATURE ext_signature_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "AS LANGUAGE MLE SIGNATURE call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_language_mle_signature RETURN NUMBER")
    );
    assert!(stmts[0].contains("AS LANGUAGE MLE SIGNATURE ext_signature_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_mle_signature_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_language_mle_signature RETURN NUMBER
AS LANGUAGE MLE SIGNATURE ext_signature_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep AS LANGUAGE MLE SIGNATURE function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_language_mle_signature RETURN NUMBER")
    );
    assert!(stmts[0].contains("AS LANGUAGE MLE SIGNATURE ext_signature_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_mle_module_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER
AS MLE MODULE ext_mle_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "AS MLE MODULE call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE MODULE ext_mle_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_mle_module_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER
AS MLE MODULE ext_mle_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep AS MLE MODULE function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE MODULE ext_mle_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_mle_signature_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_signature RETURN NUMBER
AS MLE SIGNATURE ext_signature_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "AS MLE SIGNATURE call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_signature RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE SIGNATURE ext_signature_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_mle_env_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER
AS MLE ENV ext_env_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "AS MLE ENV call spec without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE ENV ext_env_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_mle_env_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER
AS MLE ENV ext_env_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep AS MLE ENV function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE ENV ext_env_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_mle_signature_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_mle_signature RETURN NUMBER
AS MLE SIGNATURE ext_signature_impl;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep AS MLE SIGNATURE function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_signature RETURN NUMBER"));
    assert!(stmts[0].contains("AS MLE SIGNATURE ext_signature_impl"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_parameters_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_params RETURN NUMBER
AS LANGUAGE C PARAMETERS (CONTEXT);
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE ... PARAMETERS call spec function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_params RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C PARAMETERS (CONTEXT)"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_agent_in_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_agent RETURN NUMBER
AS LANGUAGE C AGENT IN extproc_agent;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE ... AGENT IN without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_agent RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C AGENT IN extproc_agent"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_agent_in_without_external_keyword_splits() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_agent RETURN NUMBER
AS LANGUAGE C AGENT IN extproc_agent;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE ... AGENT IN function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_agent RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE C AGENT IN extproc_agent"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_national_single_quoted_target_without_external_keyword_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_nquoted RETURN NUMBER
AS LANGUAGE N'C' NAME 'ext_lang_nquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE N'...' without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_nquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE N'C' NAME 'ext_lang_nquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_national_single_quoted_target_without_external_keyword_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_nquoted RETURN NUMBER
AS LANGUAGE N'C' NAME 'ext_lang_nquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE N'...' function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_nquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE N'C' NAME 'ext_lang_nquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_binary_single_quoted_target_without_external_keyword_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_bquoted RETURN NUMBER
AS LANGUAGE B'C' NAME 'ext_lang_bquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE B'...' without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_bquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE B'C' NAME 'ext_lang_bquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_binary_single_quoted_target_without_external_keyword_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_bquoted RETURN NUMBER
AS LANGUAGE B'C' NAME 'ext_lang_bquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE B'...' function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_bquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE B'C' NAME 'ext_lang_bquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_external_function_language_hex_single_quoted_target_without_external_keyword_splits()
{
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_xquoted RETURN NUMBER
AS LANGUAGE X'C' NAME 'ext_lang_xquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LANGUAGE X'...' without EXTERNAL keyword should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_xquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE X'C' NAME 'ext_lang_xquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_external_language_hex_single_quoted_target_without_external_keyword_splits(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_xquoted RETURN NUMBER
AS LANGUAGE X'C' NAME 'ext_lang_xquoted';
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep LANGUAGE X'...' function together and split trailing SELECT: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_xquoted RETURN NUMBER"));
    assert!(stmts[0].contains("AS LANGUAGE X'C' NAME 'ext_lang_xquoted'"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_function() {
    let sql = r#"CREATE FUNCTION add_nums(a NUMBER, b NUMBER) RETURN NUMBER IS
BEGIN
  RETURN a + b;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_package_spec() {
    let sql = r#"CREATE PACKAGE test_pkg AS
  PROCEDURE proc1;
  FUNCTION func1 RETURN NUMBER;
END test_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(stmts[0].contains("CREATE PACKAGE"));
    assert!(stmts[0].contains("END test_pkg"));
}

#[test]
fn test_package_spec_forward_declaration_followed_by_subtype_splits_before_next_statement() {
    let sql = r#"CREATE OR REPLACE PACKAGE test_pkg AS
  PROCEDURE proc1;
  SUBTYPE vc30 IS VARCHAR2(30);
END test_pkg;

SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "package spec with forward declaration + SUBTYPE should split before trailing SELECT: {:?}",
        stmts
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE PACKAGE test_pkg AS"),
        "first statement should preserve package spec: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SUBTYPE vc30 IS VARCHAR2(30);"),
        "first statement should preserve SUBTYPE declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_format_items_package_spec_forward_declaration_followed_by_subtype_splits_before_next_statement(
) {
    let sql = r#"CREATE OR REPLACE PACKAGE test_pkg AS
  PROCEDURE proc1;
  SUBTYPE vc30 IS VARCHAR2(30);
END test_pkg;

SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should split package spec and trailing SELECT separately: {:?}",
        stmts
    );
    assert!(stmts[0].starts_with("CREATE OR REPLACE PACKAGE test_pkg AS"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_package_body_simple() {
    let sql = r#"CREATE PACKAGE BODY test_pkg AS
  PROCEDURE proc1 IS
  BEGIN
NULL;
  END;
END test_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_package_body_complex() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_pkg AS
  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_n1 IN NUMBER DEFAULT NULL) IS
  BEGIN
INSERT INTO oqt_call_log(id, tag, msg, n1, created_at)
VALUES (oqt_call_log_seq.NEXTVAL, p_tag, p_msg, p_n1, SYSDATE);
  END;

  PROCEDURE p_basic(
p_in_num   IN  NUMBER,
p_in_txt   IN  VARCHAR2 DEFAULT 'DEF',
p_out_txt  OUT VARCHAR2,
p_inout_n  IN OUT NUMBER
  ) IS
  BEGIN
p_out_txt := 'IN_NUM='||p_in_num||', IN_TXT='||p_in_txt||', INOUT='||p_inout_n;
p_inout_n := NVL(p_inout_n,0) + p_in_num;

log_msg('P_BASIC', p_out_txt, p_in_num);
DBMS_OUTPUT.PUT_LINE('[p_basic] out='||p_out_txt||' / inout='||p_inout_n);
  END;

  PROCEDURE p_over(p_txt IN VARCHAR2) IS
  BEGIN
log_msg('P_OVER1', p_txt);
DBMS_OUTPUT.PUT_LINE('[p_over(txt)] '||NVL(p_txt,'<NULL>'));
  END;

  PROCEDURE p_over(p_num IN NUMBER, p_txt IN VARCHAR2) IS
  BEGIN
log_msg('P_OVER2', p_txt, p_num);
DBMS_OUTPUT.PUT_LINE('[p_over(num,txt)] '||p_num||' / '||NVL(p_txt,'<NULL>'));
  END;

  PROCEDURE p_refcur(p_tag IN VARCHAR2, p_rc OUT SYS_REFCURSOR) IS
  BEGIN
OPEN p_rc FOR
  SELECT id, tag, msg, n1, created_at
  FROM oqt_call_log
  WHERE tag LIKE p_tag||'%'
  ORDER BY id DESC;
  END;

  PROCEDURE p_raise(p_mode IN VARCHAR2) IS
  BEGIN
IF p_mode = 'NO_DATA_FOUND' THEN
  DECLARE v NUMBER;
  BEGIN
    SELECT n1 INTO v FROM oqt_call_log WHERE id = -9999;
  END;
ELSIF p_mode = 'APP' THEN
  RAISE_APPLICATION_ERROR(-20001, 'oqt_pkg.p_raise app error');
ELSE
  DBMS_OUTPUT.PUT_LINE('[p_raise] ok');
END IF;
  END;

  FUNCTION f_sum(p_a IN NUMBER, p_b IN NUMBER) RETURN NUMBER IS
  BEGIN
RETURN NVL(p_a,0) + NVL(p_b,0);
  END;

  FUNCTION f_echo(p_txt IN VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
RETURN 'ECHO:'||p_txt;
  END;

  FUNCTION f_dateadd(p_d IN DATE, p_days IN NUMBER DEFAULT 1) RETURN DATE IS
  BEGIN
RETURN p_d + p_days;
  END;
END oqt_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement, got {} statements",
        stmts.len()
    );
    if stmts.len() > 1 {
        for (i, s) in stmts.iter().enumerate() {
            println!("Statement {}: {}", i, &s[..s.len().min(100)]);
        }
    }
    assert!(stmts[0].contains("CREATE OR REPLACE PACKAGE BODY"));
    assert!(stmts[0].contains("END oqt_pkg"));
}

#[test]
fn test_nested_begin_end_in_package() {
    let sql = r#"CREATE PACKAGE BODY test_pkg AS
  PROCEDURE proc1 IS
  BEGIN
IF TRUE THEN
  BEGIN
    NULL;
  END;
END IF;
  END;
END test_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_if_condition_with_inline_comment_before_then() {
    let sql = r#"BEGIN
  IF (1 = 1) /* inline comment */ THEN
    NULL;
  END IF;
END;
/"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        1,
        "inline comment between IF condition and THEN should keep block depth balanced"
    );
}

#[test]
fn test_package_with_nested_declare() {
    let sql = r#"CREATE PACKAGE BODY test_pkg AS
  PROCEDURE proc1 IS
  BEGIN
DECLARE
  v_temp NUMBER;
BEGIN
  v_temp := 1;
END;
  END;
END test_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_package_body_subprogram_declare_end_label_depth() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY test_pkg AS
  FUNCTION f1 RETURN NUMBER IS
    v_temp NUMBER := 0;
  BEGIN
    DECLARE
      v_inner NUMBER := 1;
    BEGIN
      IF v_inner = 1 THEN
        v_temp := v_inner;
      END IF;
    END;
    RETURN v_temp;
  END f1;
END test_pkg;

SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "package body with DECLARE/END IF/named END should split only at final END: {:?}",
        stmts
    );
    assert!(stmts[0].contains("END IF;"));
    assert!(stmts[0].contains("END f1;"));
    assert!(stmts[0].contains("END test_pkg"));
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_package_followed_by_select() {
    let sql = r#"CREATE PACKAGE test_pkg AS
  PROCEDURE proc1;
END test_pkg;

SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "Should have 2 statements, got: {:?}", stmts);
    assert!(stmts[0].contains("CREATE PACKAGE"));
    assert!(stmts[1].contains("SELECT"));
}

#[test]
fn test_multiple_packages() {
    let sql = r#"CREATE PACKAGE pkg1 AS
  PROCEDURE proc1;
END pkg1;

CREATE PACKAGE pkg2 AS
  PROCEDURE proc2;
END pkg2;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "Should have 2 statements, got: {:?}", stmts);
}

#[test]
fn test_create_trigger() {
    let sql = r#"CREATE TRIGGER test_trg
BEFORE INSERT ON test_table
FOR EACH ROW
BEGIN
  :NEW.created_at := SYSDATE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_editioning_trigger() {
    let sql = r#"CREATE OR REPLACE EDITIONING TRIGGER test_trg
BEFORE INSERT ON test_table
FOR EACH ROW
BEGIN
  :NEW.created_at := SYSDATE;
END;

SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "EDITIONING TRIGGER body must stay as one statement before trailing SELECT: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("CREATE OR REPLACE EDITIONING TRIGGER"),
        "first statement should keep EDITIONING TRIGGER header: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_type() {
    let sql = r#"CREATE TYPE test_type AS OBJECT (
  id NUMBER,
  name VARCHAR2(100)
);"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_create_type_object_attribute_prefixed_create_does_not_force_split() {
    let sql = r#"CREATE OR REPLACE TYPE test_type AS OBJECT (
  create_flag NUMBER,
  id NUMBER
);
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE TYPE OBJECT attribute named CREATE_* should not trigger forced split: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("create_flag NUMBER"),
        "TYPE OBJECT statement should preserve CREATE_* attribute line: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_create_type_enum_splits_before_next_statement() {
    let sql = r#"CREATE OR REPLACE TYPE color_t AS ENUM ('RED', 'GREEN');
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE TYPE ... AS ENUM should split before trailing SELECT, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("AS ENUM ('RED', 'GREEN')"),
        "first statement should preserve ENUM declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_comments_stripped() {
    let sql = r#"-- This is a comment
SELECT 1 FROM DUAL;
-- Another comment"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(
        !stmts[0].starts_with("--"),
        "Leading comment should be stripped"
    );
}

#[test]
fn test_block_comment_stripped() {
    let sql = r#"/* Block comment */
SELECT 1 FROM DUAL;
/* Trailing comment */"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_strip_comments_removes_trailing_block_comment_same_line() {
    // Trailing block comment inline with SQL should be stripped.
    let sql = "SELECT 1 FROM dual /* trailing note */";
    let stripped = QueryExecutor::strip_comments(sql);
    assert_eq!(
        stripped, "SELECT 1 FROM dual",
        "Trailing inline block comment should be stripped, got: {stripped:?}"
    );
}

#[test]
fn test_strip_comments_removes_trailing_block_comment_own_line() {
    // Trailing block comment on its own line (after newline) should be stripped.
    let sql = "SELECT 1 FROM dual\n/* trailing note */";
    let stripped = QueryExecutor::strip_comments(sql);
    assert_eq!(
        stripped, "SELECT 1 FROM dual",
        "Trailing whole-line block comment should be stripped, got: {stripped:?}"
    );
}

#[test]
fn test_strip_comments_removes_multiple_trailing_block_comments() {
    // Multiple sequential trailing block comments should all be stripped.
    let sql = "SELECT 1 FROM dual /* note1 */ /* note2 */";
    let stripped = QueryExecutor::strip_comments(sql);
    assert_eq!(
        stripped, "SELECT 1 FROM dual",
        "Multiple trailing block comments should be stripped, got: {stripped:?}"
    );
}

#[test]
fn test_procedure_with_loop() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  FOR i IN 1..10 LOOP
DBMS_OUTPUT.PUT_LINE(i);
  END LOOP;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_procedure_with_case() {
    let sql = r#"CREATE PROCEDURE test_proc(p_val NUMBER) AS
BEGIN
  CASE p_val
WHEN 1 THEN DBMS_OUTPUT.PUT_LINE('one');
WHEN 2 THEN DBMS_OUTPUT.PUT_LINE('two');
ELSE DBMS_OUTPUT.PUT_LINE('other');
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_slash_terminator() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  NULL;
END;
/
SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "Should have 2 statements, got: {:?}", stmts);
}

#[test]
fn test_slash_terminator_after_end_without_semicolon() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  NULL;
END
/
SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "END followed by SQL*Plus slash without semicolon should still split, got: {:?}",
        stmts
    );
}

#[test]
fn test_slash_terminator_with_sqlplus_rem_comment() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  NULL;
END
/ REM execute block
SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "slash terminator with REM comment should split, got: {:?}",
        stmts
    );
}

#[test]
fn test_split_format_items_slash_terminator_with_sqlplus_remark_comment() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  NULL;
END
/ remark execute block
SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_format_items(sql);

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        statements.len(),
        2,
        "split_format_items should split slash+REMARK terminator, got: {:?}",
        statements
    );
    assert_eq!(slash_count, 1, "slash delimiter should be preserved once");
}

#[test]
fn test_split_format_items_slash_terminator_after_end_without_semicolon() {
    let sql = r#"CREATE PROCEDURE test_proc AS
BEGIN
  NULL;
END
/
SELECT 1 FROM DUAL;"#;
    let items = QueryExecutor::split_format_items(sql);

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        statements.len(),
        2,
        "split_format_items should split END + slash without semicolon, got: {:?}",
        statements
    );
    assert_eq!(slash_count, 1, "slash delimiter should be preserved once");
}

#[test]
fn test_compound_trigger_end_timing_point_without_semicolon_before_slash() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_compound_view
FOR INSERT ON test_view
COMPOUND TRIGGER
  INSTEAD OF EACH ROW IS
  BEGIN
    NULL;
  END INSTEAD OF EACH ROW
END
/
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "compound trigger with END timing point and no semicolon before slash should split, got: {:?}",
        stmts
    );
}

#[test]
fn test_compound_trigger_timing_point_without_is_splits_before_following_select() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_compound_no_is
FOR INSERT ON test_table
COMPOUND TRIGGER
  BEFORE STATEMENT
  BEGIN
    NULL;
  END BEFORE STATEMENT;
END;
SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "compound trigger timing-point without IS should split, got: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("END BEFORE STATEMENT"),
        "timing-point END without IS must remain in trigger statement: {}",
        stmts[0]
    );
}

#[test]
fn test_split_script_items_slash_line_inside_q_quote_is_not_terminator() {
    let sql = "SELECT q'[\n/\n]' AS txt FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "slash line inside q-quote must not split statements: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("q'[\n/\n]'"),
        "first statement should preserve q-quote slash line, got: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 2 FROM dual");
}

#[test]
fn test_split_format_items_slash_line_inside_q_quote_is_not_terminator() {
    let sql = "SELECT q'[\n/\n]' AS txt FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);

    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !items.iter().any(|item| matches!(item, FormatItem::Slash)),
        "slash line inside q-quote must not be parsed as format slash item"
    );
    assert_eq!(
        statements.len(),
        2,
        "expected two format statements, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("q'[\n/\n]'"),
        "first format statement should preserve q-quote slash line, got: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 2 FROM dual");
}

#[test]
fn test_complete_package_with_spec_and_body() {
    let sql = r#"CREATE OR REPLACE PACKAGE oqt_pkg AS
  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_n1 IN NUMBER DEFAULT NULL);

  PROCEDURE p_basic(
p_in_num   IN  NUMBER,
p_in_txt   IN  VARCHAR2 DEFAULT 'DEF',
p_out_txt  OUT VARCHAR2,
p_inout_n  IN OUT NUMBER
  );

  PROCEDURE p_over(p_txt IN VARCHAR2);
  PROCEDURE p_over(p_num IN NUMBER, p_txt IN VARCHAR2);

  PROCEDURE p_refcur(p_tag IN VARCHAR2, p_rc OUT SYS_REFCURSOR);

  PROCEDURE p_raise(p_mode IN VARCHAR2);

  FUNCTION f_sum(p_a IN NUMBER, p_b IN NUMBER) RETURN NUMBER;
  FUNCTION f_echo(p_txt IN VARCHAR2) RETURN VARCHAR2;
  FUNCTION f_dateadd(p_d IN DATE, p_days IN NUMBER DEFAULT 1) RETURN DATE;
END oqt_pkg;
/
SHOW ERRORS PACKAGE oqt_pkg;

CREATE OR REPLACE PACKAGE BODY oqt_pkg AS
  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_n1 IN NUMBER DEFAULT NULL) IS
  BEGIN
INSERT INTO oqt_call_log(id, tag, msg, n1, created_at)
VALUES (oqt_call_log_seq.NEXTVAL, p_tag, p_msg, p_n1, SYSDATE);
  END;

  PROCEDURE p_basic(
p_in_num   IN  NUMBER,
p_in_txt   IN  VARCHAR2 DEFAULT 'DEF',
p_out_txt  OUT VARCHAR2,
p_inout_n  IN OUT NUMBER
  ) IS
  BEGIN
p_out_txt := 'IN_NUM='||p_in_num||', IN_TXT='||p_in_txt||', INOUT='||p_inout_n;
p_inout_n := NVL(p_inout_n,0) + p_in_num;

log_msg('P_BASIC', p_out_txt, p_in_num);
DBMS_OUTPUT.PUT_LINE('[p_basic] out='||p_out_txt||' / inout='||p_inout_n);
  END;

  PROCEDURE p_over(p_txt IN VARCHAR2) IS
  BEGIN
log_msg('P_OVER1', p_txt);
DBMS_OUTPUT.PUT_LINE('[p_over(txt)] '||NVL(p_txt,'<NULL>'));
  END;

  PROCEDURE p_over(p_num IN NUMBER, p_txt IN VARCHAR2) IS
  BEGIN
log_msg('P_OVER2', p_txt, p_num);
DBMS_OUTPUT.PUT_LINE('[p_over(num,txt)] '||p_num||' / '||NVL(p_txt,'<NULL>'));
  END;

  PROCEDURE p_refcur(p_tag IN VARCHAR2, p_rc OUT SYS_REFCURSOR) IS
  BEGIN
OPEN p_rc FOR
  SELECT id, tag, msg, n1, created_at
  FROM oqt_call_log
  WHERE tag LIKE p_tag||'%'
  ORDER BY id DESC;
  END;

  PROCEDURE p_raise(p_mode IN VARCHAR2) IS
  BEGIN
IF p_mode = 'NO_DATA_FOUND' THEN
  DECLARE v NUMBER;
  BEGIN
    SELECT n1 INTO v FROM oqt_call_log WHERE id = -9999;
  END;
ELSIF p_mode = 'APP' THEN
  RAISE_APPLICATION_ERROR(-20001, 'oqt_pkg.p_raise app error');
ELSE
  DBMS_OUTPUT.PUT_LINE('[p_raise] ok');
END IF;
  END;

  FUNCTION f_sum(p_a IN NUMBER, p_b IN NUMBER) RETURN NUMBER IS
  BEGIN
RETURN NVL(p_a,0) + NVL(p_b,0);
  END;

  FUNCTION f_echo(p_txt IN VARCHAR2) RETURN VARCHAR2 IS
  BEGIN
RETURN 'ECHO:'||p_txt;
  END;

  FUNCTION f_dateadd(p_d IN DATE, p_days IN NUMBER DEFAULT 1) RETURN DATE IS
  BEGIN
RETURN p_d + p_days;
  END;
END oqt_pkg;
/
SHOW ERRORS PACKAGE BODY oqt_pkg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    // Count tool commands (SHOW ERRORS)
    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    if stmts.len() != 2 {
        println!(
            "\n=== FAILED: Expected 2 statements, got {} ===",
            stmts.len()
        );
        for (i, s) in stmts.iter().enumerate() {
            let preview = if s.len() > 100 { &s[..100] } else { s };
            println!("\n--- Statement {} ---\n{}...\n---", i, preview);
        }
    }

    assert_eq!(
        stmts.len(),
        2,
        "Should have 2 statements (package spec + body), got {}",
        stmts.len()
    );
    assert_eq!(
        tool_cmds.len(),
        2,
        "Should have 2 tool commands (SHOW ERRORS), got {}",
        tool_cmds.len()
    );

    // Verify package spec
    assert!(
        stmts[0].contains("CREATE OR REPLACE PACKAGE oqt_pkg AS"),
        "First statement should be package spec"
    );
    assert!(
        stmts[0].contains("END oqt_pkg"),
        "Package spec should end with END oqt_pkg"
    );
    assert!(
        !stmts[0].contains("PACKAGE BODY"),
        "Package spec should not contain BODY"
    );

    // Verify package body
    assert!(
        stmts[1].contains("CREATE OR REPLACE PACKAGE BODY oqt_pkg AS"),
        "Second statement should be package body"
    );
    assert!(
        stmts[1].contains("END oqt_pkg"),
        "Package body should end with END oqt_pkg"
    );
}

#[test]
fn test_show_errors_without_slash() {
    // Test case: SHOW ERRORS without preceding slash (/) separator
    // This simulates the user's issue where SHOW ERRORS is not separated
    // from the CREATE PACKAGE BODY when there's no slash terminator
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_deep_pkg AS

  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_depth IN NUMBER) IS
  BEGIN
INSERT INTO oqt_t_log(log_id, tag, msg, depth)
VALUES (oqt_seq_log.NEXTVAL, SUBSTR(p_tag,1,30), SUBSTR(p_msg,1,4000), p_depth);
DBMS_OUTPUT.PUT_LINE('[LOG]['||p_tag||'][depth='||p_depth||'] '||p_msg);
  END;

END oqt_deep_pkg;

SHOW ERRORS"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    // Debug output
    println!("\n=== Test: SHOW ERRORS without slash ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, item) in items.iter().enumerate() {
        match item {
            ScriptItem::Statement(s) => {
                let preview = if s.len() > 80 {
                    format!("{}...", &s[..80])
                } else {
                    s.clone()
                };
                println!("\n[{}] Statement: {}", i, preview);
            }
            ScriptItem::ToolCommand(cmd) => {
                println!("\n[{}] ToolCommand: {:?}", i, cmd);
            }
        }
    }

    // Should have 1 statement (CREATE PACKAGE BODY) and 1 tool command (SHOW ERRORS)
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (package body), got {}",
        stmts.len()
    );
    assert_eq!(
        tool_cmds.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS), got {}",
        tool_cmds.len()
    );

    // Verify package body doesn't contain SHOW ERRORS
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package body should NOT contain SHOW ERRORS"
    );
}

#[test]
fn test_show_errors_complex_package_without_slash() {
    // Test case from user: complex package body with nested procedures,
    // CASE, LOOP, DECLARE blocks, followed by SHOW ERRORS without slash
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_deep_pkg AS

  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_depth IN NUMBER) IS
  BEGIN
INSERT INTO oqt_t_log(log_id, tag, msg, depth)
VALUES (oqt_seq_log.NEXTVAL, SUBSTR(p_tag,1,30), SUBSTR(p_msg,1,4000), p_depth);
DBMS_OUTPUT.PUT_LINE('[LOG]['||p_tag||'][depth='||p_depth||'] '||p_msg);
  END;

  FUNCTION f_calc(p_n IN NUMBER) RETURN NUMBER IS
v NUMBER := 0;
  BEGIN
-- Nested IF + CASE + inner block
IF p_n IS NULL THEN
  v := -1;
ELSE
  CASE
    WHEN p_n < 0 THEN
      v := p_n * p_n;
    WHEN p_n BETWEEN 0 AND 10 THEN
      DECLARE
        x NUMBER := p_n + 100;
      BEGIN
        v := x - 50;
      END;
    ELSE
      v := p_n + 999;
  END CASE;
END IF;

RETURN v;
  EXCEPTION
WHEN OTHERS THEN
  log_msg('F_CALC', 'error='||SQLERRM, 999);
  RETURN NULL;
  END;

  PROCEDURE p_deep_run(p_limit IN NUMBER DEFAULT 7) IS
v_depth NUMBER := 0;

PROCEDURE p_inner(p_i NUMBER, p_j NUMBER) IS
  v_local NUMBER := 0;
BEGIN
  v_depth := v_depth + 1;
  v_local := f_calc(p_i - p_j);

  <<outer_loop>>
  FOR k IN 1..3 LOOP
    v_depth := v_depth + 1;

    CASE MOD(k + p_i + p_j, 4)
      WHEN 0 THEN
        log_msg('INNER', 'case0 k='||k||' local='||v_local, v_depth);
      WHEN 1 THEN
        DECLARE
          z NUMBER := 10;
        BEGIN
          IF z = 10 THEN
            log_msg('INNER', 'case1 -> raise user error', v_depth);
            RAISE_APPLICATION_ERROR(-20001, 'forced error in inner block');
          END IF;
        EXCEPTION
          WHEN OTHERS THEN
            log_msg('INNER', 'handled inner exception: '||SQLERRM, v_depth);
        END;
      WHEN 2 THEN
        log_msg('INNER', 'case2 -> continue outer_loop', v_depth);
        CONTINUE outer_loop;
      ELSE
        log_msg('INNER', 'case3 -> exit outer_loop', v_depth);
        EXIT outer_loop;
    END CASE;

    DECLARE
      w NUMBER := 0;
    BEGIN
      WHILE w < 2 LOOP
        w := w + 1;
        log_msg('INNER', 'while w='||w, v_depth+1);
      END LOOP;
    END;

  END LOOP outer_loop;

  v_depth := v_depth - 1;
END p_inner;

  BEGIN
log_msg('P_DEEP_RUN', 'start limit='||p_limit, v_depth);

FOR r IN (SELECT id, grp, name FROM oqt_t_depth WHERE id <= p_limit ORDER BY id) LOOP
  v_depth := v_depth + 1;

  BEGIN
    IF r.grp = 0 THEN
      log_msg('RUN', 'grp=0 id='||r.id||' name='||r.name, v_depth);

      CASE
        WHEN r.id IN (1,2) THEN
          p_inner(r.id, 1);
        WHEN r.id BETWEEN 3 AND 5 THEN
          p_inner(r.id, 2);
        ELSE
          p_inner(r.id, 3);
      END CASE;

    ELSIF r.grp = 1 THEN
      log_msg('RUN', 'grp=1 id='||r.id||' (dynamic insert)', v_depth);

      EXECUTE IMMEDIATE
        'INSERT INTO oqt_t_log(log_id, tag, msg, depth)
         VALUES (oqt_seq_log.NEXTVAL, :t, :m, :d)'
      USING 'DYN', 'insert from dyn sql id='||r.id, v_depth;

    ELSE
      log_msg('RUN', 'grp=2 id='||r.id||' (raise & catch)', v_depth);
      BEGIN
        IF r.id = 6 THEN
          log_msg('RUN', 'string contains tokens: "BEGIN END; / CASE WHEN"', v_depth);
        END IF;

        IF r.id = 7 THEN
          RAISE NO_DATA_FOUND;
        END IF;

      EXCEPTION
        WHEN NO_DATA_FOUND THEN
          log_msg('RUN', 'caught NO_DATA_FOUND for id='||r.id, v_depth);
      END;
    END IF;

  EXCEPTION
    WHEN OTHERS THEN
      log_msg('RUN', 'outer exception caught: '||SQLERRM, v_depth);
  END;

  v_depth := v_depth - 1;
END LOOP;

DECLARE
  t oqt_deep_tab := oqt_deep_tab();
BEGIN
  t.EXTEND(3);
  t(1) := oqt_deep_obj(1, 'A');
  t(2) := oqt_deep_obj(2, 'B');
  t(3) := oqt_deep_obj(3, 'C');

  FOR i IN 1..t.COUNT LOOP
    log_msg('COLL', 't('||i||')='||t(i).k||','||t(i).v, 77);
  END LOOP;
END;

log_msg('P_DEEP_RUN', 'done', v_depth);
  END p_deep_run;

END oqt_deep_pkg;

SHOW ERRORS

--------------------------------------------------------------------------------
PROMPT [5] REFCURSOR test (VARIABLE/PRINT + OUT refcursor)
--------------------------------------------------------------------------------

VAR v_rc REFCURSOR"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    // Debug output
    println!("\n=== Test: Complex package body with SHOW ERRORS (no slash) ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, item) in items.iter().enumerate() {
        match item {
            ScriptItem::Statement(s) => {
                let preview = if s.len() > 120 {
                    format!("{}...", &s[..120])
                } else {
                    s.clone()
                };
                println!("\n[{}] Statement (len={}): {}", i, s.len(), preview);
            }
            ScriptItem::ToolCommand(cmd) => {
                println!("\n[{}] ToolCommand: {:?}", i, cmd);
            }
        }
    }

    // Should have 1 statement (CREATE PACKAGE BODY)
    // Tool commands: SHOW ERRORS, PROMPT, VAR
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (package body), got {}",
        stmts.len()
    );

    // Verify package body doesn't contain SHOW ERRORS
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package body should NOT contain SHOW ERRORS - it was not separated!"
    );

    // Should have at least SHOW ERRORS and VAR commands
    assert!(
        tool_cmds.len() >= 2,
        "Should have at least 2 tool commands (SHOW ERRORS, VAR), got {}",
        tool_cmds.len()
    );
}

#[test]
fn test_show_errors_with_ref_cursor_procedure() {
    // Additional test: package body with REF CURSOR procedure
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_deep_pkg AS

  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_depth IN NUMBER) IS
  BEGIN
INSERT INTO oqt_t_log(log_id, tag, msg, depth)
VALUES (oqt_seq_log.NEXTVAL, SUBSTR(p_tag,1,30), SUBSTR(p_msg,1,4000), p_depth);
  END;

  PROCEDURE p_open_rc(p_grp IN NUMBER, p_rc OUT t_rc) IS
v_sql VARCHAR2(32767);
  BEGIN
-- Dynamic SQL + bind
v_sql := 'SELECT id, grp, name, created_at
          FROM oqt_t_depth
          WHERE grp = :b1
          ORDER BY id';

OPEN p_rc FOR v_sql USING p_grp;
log_msg('P_OPEN_RC', 'opened rc for grp='||p_grp, 1);
  END;

END oqt_deep_pkg;

SHOW ERRORS"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    println!("\n=== Test: Package with REF CURSOR procedure ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, item) in items.iter().enumerate() {
        match item {
            ScriptItem::Statement(s) => {
                println!("\n[{}] Statement (len={}):\n{}", i, s.len(), s);
            }
            ScriptItem::ToolCommand(cmd) => {
                println!("\n[{}] ToolCommand: {:?}", i, cmd);
            }
        }
    }

    // Should have 1 statement and 1 tool command
    assert_eq!(stmts.len(), 1, "Should have 1 statement");
    assert_eq!(tool_cmds.len(), 1, "Should have 1 tool command");
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package body should NOT contain SHOW ERRORS"
    );
}

#[test]
fn test_package_body_show_errors_without_slash_newline_only() {
    // Test case matching user's exact issue:
    // Package body ends with "END package_name;" and newlines,
    // then SHOW ERRORS without a preceding slash
    //
    // Full test with IF, CASE, DECLARE block, and IS NULL expression
    let sql = "CREATE OR REPLACE PACKAGE BODY oqt_deep_pkg AS

  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_depth IN NUMBER) IS
  BEGIN
DBMS_OUTPUT.PUT_LINE('[LOG]['||p_tag||'][depth='||p_depth||'] '||p_msg);
  END;

  FUNCTION f_calc(p_n IN NUMBER) RETURN NUMBER IS
v NUMBER := 0;
  BEGIN
IF p_n IS NULL THEN
  v := -1;
ELSE
  CASE
    WHEN p_n < 0 THEN
      v := p_n * p_n;
    WHEN p_n BETWEEN 0 AND 10 THEN
      DECLARE
        x NUMBER := p_n + 100;
      BEGIN
        v := x - 50;
      END;
    ELSE
      v := p_n + 999;
  END CASE;
END IF;
RETURN v;
  EXCEPTION
WHEN OTHERS THEN
  log_msg('F_CALC', 'error='||SQLERRM, 999);
  RETURN NULL;
  END;

END oqt_deep_pkg;

SHOW ERRORS";

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    println!("\n=== Test: Package body + SHOW ERRORS without slash (newline only) ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, item) in items.iter().enumerate() {
        match item {
            ScriptItem::Statement(s) => {
                let lines: Vec<&str> = s.lines().collect();
                let last_lines = if lines.len() > 5 {
                    lines[lines.len() - 5..].join("\n")
                } else {
                    s.clone()
                };
                println!(
                    "\n[{}] Statement (len={}, lines={}):\n...last lines:\n{}",
                    i,
                    s.len(),
                    lines.len(),
                    last_lines
                );
            }
            ScriptItem::ToolCommand(cmd) => {
                println!("\n[{}] ToolCommand: {:?}", i, cmd);
            }
        }
    }

    // Should have 1 statement and 1 tool command
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (package body), got {}",
        stmts.len()
    );
    assert_eq!(
        tool_cmds.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS), got {}",
        tool_cmds.len()
    );

    // Verify package body doesn't contain SHOW ERRORS
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package body should NOT contain SHOW ERRORS - statement was not properly separated!"
    );
}

#[test]
fn test_package_spec_ends_with_depth_zero() {
    // Test case: Package SPEC (not BODY) should end with depth 0
    // Package spec has AS/IS but no BEGIN, ends with END package_name;
    let sql = r#"CREATE OR REPLACE PACKAGE oqt_deep_pkg AS
  -- REFCURSOR type
  TYPE t_rc IS REF CURSOR;

  -- simple log
  PROCEDURE log_msg(p_tag IN VARCHAR2, p_msg IN VARCHAR2, p_depth IN NUMBER);

  -- returns scalar with nested control flows
  FUNCTION f_calc(p_n IN NUMBER) RETURN NUMBER;

  -- opens refcursor with dynamic SQL and returns it via OUT
  PROCEDURE p_open_rc(p_grp IN NUMBER, p_rc OUT t_rc);

  -- heavy nested block for depth/parsing test
  PROCEDURE p_deep_run(p_limit IN NUMBER DEFAULT 7);
END oqt_deep_pkg;

SHOW ERRORS"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    println!("\n=== Test: Package SPEC ends with depth 0 ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, item) in items.iter().enumerate() {
        match item {
            ScriptItem::Statement(s) => {
                println!("\n[{}] Statement (len={}):\n{}", i, s.len(), s);
            }
            ScriptItem::ToolCommand(cmd) => {
                println!("\n[{}] ToolCommand: {:?}", i, cmd);
            }
        }
    }

    // Should have 1 statement (package spec) and 1 tool command (SHOW ERRORS)
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (package spec), got {}",
        stmts.len()
    );
    assert_eq!(
        tool_cmds.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS), got {}",
        tool_cmds.len()
    );

    // Verify package spec doesn't contain SHOW ERRORS
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package spec should NOT contain SHOW ERRORS - depth did not return to 0!"
    );
}

#[test]
fn test_split_format_items_package_body_then_show_errors_prompt_trigger_after_consecutive_case_ends(
) {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_mega_pkg AS
  FUNCTION f_deep RETURN NUMBER IS
    v NUMBER := 0;
  BEGIN
    v :=
      CASE
        WHEN 1 = 1 THEN
          CASE
            WHEN 2 = 2 THEN 100
            ELSE 10
          END
        ELSE
          CASE
            WHEN 3 = 3 THEN 777
            ELSE 0
          END
      END;
    RETURN v;
  END;

  PROCEDURE run_torture IS
  BEGIN
    NULL;
  END run_torture;
END oqt_mega_pkg;
/
SHOW ERRORS PACKAGE BODY oqt_mega_pkg

PROMPT [6] trigger

CREATE OR REPLACE TRIGGER oqt_trg_test_bi
BEGIN
  NULL;
END;
/
BEGIN
  NULL;
END;
/"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        stmts.len(),
        3,
        "expected package, trigger, and block statements: {items:?}"
    );
    assert_eq!(
        slash_count, 3,
        "slash delimiters should be preserved around each block: {items:?}"
    );
    assert!(
        matches!(&items[1], FormatItem::Slash),
        "package body should terminate on its own slash before SHOW ERRORS: {items:?}"
    );
    assert!(
        matches!(
            &items[2],
            FormatItem::ToolCommand(ToolCommand::ShowErrors { .. })
        ),
        "SHOW ERRORS PACKAGE BODY should be a standalone tool command: {items:?}"
    );
    assert!(
        matches!(&items[3], FormatItem::Verbatim(text) if text == "PROMPT [6] trigger"),
        "PROMPT after SHOW ERRORS should remain a standalone verbatim format item: {items:?}"
    );
    assert!(
        stmts[0].contains("END oqt_mega_pkg")
            && !stmts[0].contains("SHOW ERRORS")
            && !stmts[0].contains("PROMPT [6]")
            && !stmts[0].contains("CREATE OR REPLACE TRIGGER"),
        "package body statement should not absorb following report/prompt/trigger lines: {}",
        stmts[0]
    );
}

#[test]
fn test_package_body_with_declare_blocks() {
    // Test case: Package body with nested procedure
    // This is the minimal case that fails
    let sql = r#"CREATE OR REPLACE PACKAGE BODY test_pkg AS
  PROCEDURE p_outer IS
PROCEDURE p_inner IS
BEGIN
  NULL;
END p_inner;
  BEGIN
NULL;
  END p_outer;
END test_pkg;

SHOW ERRORS"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    println!("\n=== Test: Package body with DECLARE blocks ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());
    println!("Tool commands: {}", tool_cmds.len());

    for (i, stmt) in stmts.iter().enumerate() {
        println!("\n[{}] Statement:\n{}", i, stmt);
    }

    assert_eq!(stmts.len(), 1, "Should have 1 statement");
    assert_eq!(tool_cmds.len(), 1, "Should have 1 tool command");
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "Package body should NOT contain SHOW ERRORS"
    );
}

#[test]
fn test_anonymous_block_with_nested_procedure() {
    // Test case: Anonymous block with nested procedure declaration
    // The nested DECLARE inside labeled block should not split the statement
    let sql = r#"DECLARE
  v NUMBER := 0;
  PROCEDURE bump(p IN OUT NUMBER) IS
  BEGIN
p := p + 1;
  END;
BEGIN
  <<blk1>>
  DECLARE
a NUMBER := 0;
  BEGIN
FOR i IN 1..3 LOOP
  bump(a);
END LOOP;
  END blk1;
EXCEPTION
  WHEN OTHERS THEN
DBMS_OUTPUT.PUT_LINE('[ANON] top exception handled: '||SQLERRM);
END;"#;

    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    println!("\n=== Test: Anonymous block with nested procedure ===");
    println!("Total items: {}", items.len());
    println!("Statements: {}", stmts.len());

    for (i, stmt) in stmts.iter().enumerate() {
        println!("\n[{}] Statement (len={}):\n{}", i, stmt.len(), stmt);
    }

    // Should be exactly 1 statement (the entire anonymous block)
    assert_eq!(
        stmts.len(),
        1,
        "Should have exactly 1 statement (anonymous block), got {}. Block was incorrectly split!",
        stmts.len()
    );

    // Verify the statement contains both the procedure and the call
    assert!(
        stmts[0].contains("PROCEDURE bump"),
        "Statement should contain PROCEDURE bump declaration"
    );
    assert!(
        stmts[0].contains("bump(a)"),
        "Statement should contain bump(a) call"
    );
}

#[test]
fn test_select_with_case_when_expression() {
    // Test case: SELECT with CASE WHEN ... END expression
    // The CASE expression END should NOT be treated as a PL/SQL block END
    let sql = "SELECT CASE WHEN 1=1 THEN 'Y' ELSE 'N' END FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(
        stmts[0].contains("CASE WHEN"),
        "Statement should contain CASE WHEN"
    );
}

#[test]
fn test_select_with_case_when_as_alias() {
    // Test case: SELECT with CASE WHEN ... END AS alias
    let sql = "SELECT CASE WHEN 1=1 THEN 'Y' ELSE 'N' END AS result FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_select_with_multiple_case_expressions() {
    // Test case: SELECT with multiple CASE expressions
    let sql = "SELECT CASE WHEN a=1 THEN 'one' END, CASE WHEN b=2 THEN 'two' END FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_plsql_block_with_case_expression_select() {
    // Test case: PL/SQL block containing SELECT with CASE expression
    // This is the critical case where block_depth could be incorrectly decremented
    let sql = r#"BEGIN
  SELECT CASE WHEN 1=1 THEN 'Y' ELSE 'N' END INTO v_result FROM dual;
  NULL;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (entire PL/SQL block), got: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("NULL"),
        "Statement should contain NULL (proving block wasn't split)"
    );
}

#[test]
fn test_procedure_with_case_expression_in_select() {
    // Test case: CREATE PROCEDURE with SELECT containing CASE expression
    let sql = r#"CREATE PROCEDURE test_proc AS
  v_result VARCHAR2(1);
BEGIN
  SELECT CASE WHEN 1=1 THEN 'Y' ELSE 'N' END INTO v_result FROM dual;
  DBMS_OUTPUT.PUT_LINE(v_result);
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_nested_case_expressions() {
    // Test case: Nested CASE expressions
    let sql =
        "SELECT CASE WHEN a=1 THEN CASE WHEN b=2 THEN 'A' ELSE 'B' END ELSE 'C' END FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_case_statement_vs_case_expression() {
    // Test case: PL/SQL CASE statement (with END CASE) vs CASE expression (with just END)
    let sql = r#"BEGIN
  -- CASE expression in SELECT
  SELECT CASE WHEN 1=1 THEN 'Y' END INTO v_val FROM dual;
  -- CASE statement (PL/SQL control flow)
  CASE v_val
WHEN 'Y' THEN DBMS_OUTPUT.PUT_LINE('Yes');
ELSE DBMS_OUTPUT.PUT_LINE('No');
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_case_statement_with_nested_declare_begin_end() {
    // Regression: CASE statement 안의 DECLARE...BEGIN...END 블록이
    // case_depth로 잘못 소비되어 block_depth가 남는 경우
    let sql = r#"BEGIN
  CASE v_val
WHEN 'A' THEN
  DECLARE
    x NUMBER := 0;
  BEGIN
    x := 1;
  END;
ELSE
  NULL;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_case_statement_with_nested_begin_end() {
    // CASE statement 안 standalone BEGIN...END 블록
    let sql = r#"BEGIN
  CASE v_val
WHEN 1 THEN
  BEGIN
    DBMS_OUTPUT.PUT_LINE('nested');
  END;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_case_statement_with_nested_block_and_exception() {
    // test5.txt p_inner 패턴: CASE statement 안 DECLARE/BEGIN/EXCEPTION/END
    let sql = r#"BEGIN
  CASE MOD(k, 4)
WHEN 0 THEN
  NULL;
WHEN 1 THEN
  DECLARE
    z NUMBER := 10;
  BEGIN
    IF z = 10 THEN
      RAISE_APPLICATION_ERROR(-20001, 'test');
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END;
ELSE
  NULL;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_case_statement_with_case_expression_inside() {
    // CASE statement 안에 CASE expression (SELECT ... CASE ... END)이 중첩
    let sql = r#"BEGIN
  CASE v_val
WHEN 1 THEN
  SELECT CASE WHEN x=1 THEN 'A' ELSE 'B' END INTO v_res FROM dual;
ELSE
  NULL;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_multiple_case_statements_in_sequence() {
    // 연속 CASE statement 두 개 + 중첩 블록
    let sql = r#"BEGIN
  CASE v1
WHEN 1 THEN
  BEGIN
    NULL;
  END;
  END CASE;
  CASE v2
WHEN 2 THEN
  BEGIN
    NULL;
  END;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_nested_case_statements() {
    // CASE statement 안에 CASE statement 중첩 (각각 내부 블록 포함)
    let sql = r#"BEGIN
  CASE v1
WHEN 1 THEN
  CASE v2
    WHEN 'A' THEN
      BEGIN
        NULL;
      END;
  END CASE;
  END CASE;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_basic() {
    // Basic COMPOUND TRIGGER with single timing point
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR INSERT ON test_table
COMPOUND TRIGGER
  BEFORE STATEMENT IS
  BEGIN
DBMS_OUTPUT.PUT_LINE('Before statement');
  END BEFORE STATEMENT;
END test_compound_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_multiple_timing_points() {
    // COMPOUND TRIGGER with all four timing points
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR INSERT OR UPDATE ON test_table
COMPOUND TRIGGER
  v_count NUMBER := 0;

  BEFORE STATEMENT IS
  BEGIN
v_count := 0;
  END BEFORE STATEMENT;

  BEFORE EACH ROW IS
  BEGIN
v_count := v_count + 1;
  END BEFORE EACH ROW;

  AFTER EACH ROW IS
  BEGIN
DBMS_OUTPUT.PUT_LINE('Row ' || v_count);
  END AFTER EACH ROW;

  AFTER STATEMENT IS
  BEGIN
DBMS_OUTPUT.PUT_LINE('Total: ' || v_count);
  END AFTER STATEMENT;
END test_compound_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_with_declare_in_timing_point() {
    // COMPOUND TRIGGER with local declarations in timing points
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR INSERT ON test_table
COMPOUND TRIGGER
  BEFORE EACH ROW IS
v_local NUMBER;
  BEGIN
v_local := 1;
:NEW.col1 := v_local;
  END BEFORE EACH ROW;
END test_compound_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_with_nested_blocks() {
    // COMPOUND TRIGGER with nested BEGIN/END blocks inside timing points
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR INSERT ON test_table
COMPOUND TRIGGER
  AFTER EACH ROW IS
  BEGIN
IF :NEW.status = 'ACTIVE' THEN
  BEGIN
    INSERT INTO audit_table VALUES (:NEW.id, SYSDATE);
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END;
END IF;
  END AFTER EACH ROW;
END test_compound_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_followed_by_show_errors() {
    // COMPOUND TRIGGER followed by SHOW ERRORS should be separate
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR INSERT ON test_table
COMPOUND TRIGGER
  BEFORE STATEMENT IS
  BEGIN
NULL;
  END BEFORE STATEMENT;
END test_compound_trg;

SHOW ERRORS"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();
    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();
    assert_eq!(stmts.len(), 1, "Should have 1 statement");
    assert_eq!(
        tool_cmds.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS)"
    );
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "COMPOUND TRIGGER should NOT contain SHOW ERRORS"
    );
}

#[test]
fn test_compound_trigger_with_case_statement() {
    // COMPOUND TRIGGER with CASE statement inside timing point
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR UPDATE ON test_table
COMPOUND TRIGGER
  AFTER EACH ROW IS
  BEGIN
CASE :NEW.type
  WHEN 'A' THEN
    INSERT INTO log_a VALUES (:NEW.id);
  WHEN 'B' THEN
    INSERT INTO log_b VALUES (:NEW.id);
  ELSE
    NULL;
END CASE;
  END AFTER EACH ROW;
END test_compound_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_instead_of_each_row() {
    // COMPOUND TRIGGER can use INSTEAD OF EACH ROW timing point for views.
    // END INSTEAD OF EACH ROW should close only timing-point depth.
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_view_trg
INSTEAD OF INSERT ON test_view
COMPOUND TRIGGER
  INSTEAD OF EACH ROW IS
  BEGIN
    INSERT INTO base_table(id) VALUES (:NEW.id);
  END INSTEAD OF EACH ROW;
END test_compound_view_trg;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_compound_trigger_instead_of_followed_by_show_errors() {
    // Ensure END INSTEAD OF ... does not leave depth stale and swallow next command.
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_view_trg
INSTEAD OF INSERT ON test_view
COMPOUND TRIGGER
  INSTEAD OF EACH ROW IS
  BEGIN
    NULL;
  END INSTEAD OF EACH ROW;
END test_compound_view_trg;

SHOW ERRORS"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();
    let tool_cmds: Vec<_> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();
    assert_eq!(stmts.len(), 1, "Should have 1 statement");
    assert_eq!(
        tool_cmds.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS)"
    );
    assert!(
        !stmts[0].contains("SHOW ERRORS"),
        "COMPOUND TRIGGER should NOT contain SHOW ERRORS"
    );
}

#[test]
fn test_compound_trigger_after_statement_splits_following_statement() {
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_after_stmt
FOR UPDATE ON test_tab
COMPOUND TRIGGER
  AFTER STATEMENT IS
  BEGIN
    NULL;
  END AFTER STATEMENT;
END test_compound_after_stmt;

SELECT 1 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "compound trigger should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].contains("END AFTER STATEMENT"),
        "first statement should keep timing-point END: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 1 FROM dual");
}

#[test]
fn test_create_view_with_subqueries_and_like_patterns() {
    // CREATE VIEW with:
    // - Subqueries in CASE WHEN (SELECT ... IN (subquery))
    // - Scalar subquery with COUNT(*)
    // - LIKE patterns containing ';', 'END;', '/ ' (could be misinterpreted)
    // - Multiple nested parentheses and IN clauses
    let sql = r#"CREATE OR REPLACE VIEW oqt_nm_v AS
SELECT
  t.id,
  t.grp,
  CASE
WHEN t.id IN (SELECT id FROM oqt_nm_t WHERE id <= 9) THEN 'IN'
ELSE 'OUT'
  END AS flag,
  (SELECT COUNT(*)
 FROM oqt_nm_t x
WHERE x.grp=t.grp
  AND (x.payload LIKE '%;%' OR x.payload LIKE '%END;%' OR x.payload LIKE '%/ %')
  ) AS cnt_like
FROM oqt_nm_t t
WHERE (t.id BETWEEN 1 AND 999999)
  AND ( (t.grp IN ('G0','G1','G2')) OR (t.grp IN ('G3','G4','G5','G6')) );"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(stmts[0].starts_with("CREATE OR REPLACE VIEW"));
    assert!(stmts[0].contains("cnt_like"));
}

#[test]
fn test_create_view_without_trailing_semicolon() {
    // Same CREATE VIEW but without trailing semicolon
    let sql = r#"CREATE OR REPLACE VIEW oqt_nm_v AS
SELECT
  t.id,
  t.grp,
  CASE
WHEN t.id IN (SELECT id FROM oqt_nm_t WHERE id <= 9) THEN 'IN'
ELSE 'OUT'
  END AS flag,
  (SELECT COUNT(*)
 FROM oqt_nm_t x
WHERE x.grp=t.grp
  AND (x.payload LIKE '%;%' OR x.payload LIKE '%END;%' OR x.payload LIKE '%/ %')
  ) AS cnt_like
FROM oqt_nm_t t
WHERE (t.id BETWEEN 1 AND 999999)
  AND ( (t.grp IN ('G0','G1','G2')) OR (t.grp IN ('G3','G4','G5','G6')) )"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
    assert!(stmts[0].starts_with("CREATE OR REPLACE VIEW"));
    assert!(stmts[0].contains("cnt_like"));
}

#[test]
fn test_create_view_followed_by_another_statement() {
    // CREATE VIEW followed by another statement
    let sql = r#"CREATE OR REPLACE VIEW oqt_nm_v AS
SELECT
  t.id,
  t.grp,
  CASE
WHEN t.id IN (SELECT id FROM oqt_nm_t WHERE id <= 9) THEN 'IN'
ELSE 'OUT'
  END AS flag,
  (SELECT COUNT(*)
 FROM oqt_nm_t x
WHERE x.grp=t.grp
  AND (x.payload LIKE '%;%' OR x.payload LIKE '%END;%' OR x.payload LIKE '%/ %')
  ) AS cnt_like
FROM oqt_nm_t t
WHERE (t.id BETWEEN 1 AND 999999)
  AND ( (t.grp IN ('G0','G1','G2')) OR (t.grp IN ('G3','G4','G5','G6')) );

SELECT * FROM oqt_nm_v;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "Should have 2 statements, got: {:?}", stmts);
    assert!(stmts[0].starts_with("CREATE OR REPLACE VIEW"));
    assert!(stmts[0].contains("cnt_like"));
    assert!(stmts[1].contains("SELECT * FROM oqt_nm_v"));
}

#[test]
fn test_create_view_with_slash_terminator() {
    // CREATE VIEW terminated by "/" instead of ";"
    let sql = r#"CREATE OR REPLACE VIEW oqt_nm_v AS
SELECT
  t.id,
  t.grp,
  CASE
WHEN t.id IN (SELECT id FROM oqt_nm_t WHERE id <= 9) THEN 'IN'
ELSE 'OUT'
  END AS flag,
  (SELECT COUNT(*)
 FROM oqt_nm_t x
WHERE x.grp=t.grp
  AND (x.payload LIKE '%;%' OR x.payload LIKE '%END;%' OR x.payload LIKE '%/ %')
  ) AS cnt_like
FROM oqt_nm_t t
WHERE (t.id BETWEEN 1 AND 999999)
  AND ( (t.grp IN ('G0','G1','G2')) OR (t.grp IN ('G3','G4','G5','G6')) )
/

SELECT * FROM oqt_nm_v;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "Should have 2 statements, got: {:?}", stmts);
    assert!(stmts[0].starts_with("CREATE OR REPLACE VIEW"));
    assert!(stmts[0].contains("cnt_like"));
    assert!(stmts[1].contains("SELECT * FROM oqt_nm_v"));
}

#[test]
fn test_extract_bind_names_skips_new_old_in_trigger() {
    // CREATE TRIGGER should NOT extract :NEW and :OLD as bind variables
    let sql = r#"CREATE OR REPLACE TRIGGER test_trg
BEFORE INSERT ON test_table
FOR EACH ROW
BEGIN
  :NEW.created_at := SYSDATE;
  :NEW.created_by := :user_id;
  IF :OLD.status IS NOT NULL THEN
:NEW.modified_at := SYSDATE;
  END IF;
END;"#;
    let names = QueryExecutor::extract_bind_names(sql);
    // :NEW and :OLD should be skipped, only :user_id should be extracted
    assert_eq!(
        names.len(),
        1,
        "Should have 1 bind variable, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "USER_ID"),
        "Should contain USER_ID, got: {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.to_uppercase() == "NEW"),
        "Should NOT contain NEW, got: {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.to_uppercase() == "OLD"),
        "Should NOT contain OLD, got: {:?}",
        names
    );
}

#[test]
fn test_extract_bind_names_normal_plsql_includes_new_old() {
    // Regular PL/SQL block (not CREATE TRIGGER) should extract :NEW and :OLD as bind variables
    let sql = r#"BEGIN
  :NEW := 'test';
  :OLD := 'old_value';
END;"#;
    let names = QueryExecutor::extract_bind_names(sql);
    // Both :NEW and :OLD should be extracted as they are regular bind variables here
    assert_eq!(
        names.len(),
        2,
        "Should have 2 bind variables, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "NEW"),
        "Should contain NEW, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "OLD"),
        "Should contain OLD, got: {:?}",
        names
    );
}

#[test]
fn test_extract_bind_names_supports_quoted_bind_names() {
    let sql = r#"INSERT INTO t(id, name)
VALUES (:int_val, :str_val)
RETURNING id, name INTO :"_val1", :"VaL_2""#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names,
        vec![
            "INT_VAL".to_string(),
            "STR_VAL".to_string(),
            "_VAL1".to_string(),
            "VAL_2".to_string(),
        ]
    );
}

#[test]
fn test_extract_bind_names_supports_non_ascii_bind_names() {
    let sql = "INSERT INTO t(id) VALUES (:int_val) RETURNING id INTO :m\u{00E9}il";
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names,
        vec!["INT_VAL".to_string(), "M\u{00C9}IL".to_string()]
    );
}

#[test]
fn test_extract_bind_names_supports_whitespace_after_colon() {
    let sql = r#"SELECT : value, : 1, : "MiXeD" FROM dual"#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names,
        vec!["VALUE".to_string(), "1".to_string(), "MIXED".to_string()]
    );
}

#[test]
fn test_extract_bind_names_requires_alpha_start_for_named_binds() {
    let sql = r#"SELECT :_bad, :$bad, :#bad, : good_1$#, : méil, : 1, : "_quoted" FROM dual"#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names,
        vec![
            "GOOD_1$#".to_string(),
            "M\u{00C9}IL".to_string(),
            "1".to_string(),
            "_QUOTED".to_string(),
        ]
    );
}

#[test]
fn test_extract_bind_names_skips_json_key_colons() {
    let sql = r#"
SELECT JSON_OBJECT(
         'id' : : id,
         'staticNumber' : 1,
         'staticText':'value',
         'derived'::bv2,
         'nested': JSON {'count':2, 'label':'ok'}
       RETURNING JSON)
FROM dual"#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(names, vec!["ID".to_string(), "BV2".to_string()]);
}

#[test]
fn test_extract_bind_names_resets_json_string_state_on_comments_and_identifiers() {
    let sql = r#"
SELECT 'key' /* block comment */ :after_block,
       'key' -- line comment
       :after_line,
       'key' "quoted_identifier" :after_identifier
FROM dual"#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names,
        vec![
            "AFTER_BLOCK".to_string(),
            "AFTER_LINE".to_string(),
            "AFTER_IDENTIFIER".to_string(),
        ]
    );
}

#[test]
fn test_is_create_trigger() {
    // Positive cases
    assert!(QueryExecutor::is_create_trigger(
        "CREATE TRIGGER trg_test BEFORE INSERT"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "CREATE OR REPLACE TRIGGER trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "create or replace trigger trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "CREATE EDITIONABLE TRIGGER trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "CREATE OR REPLACE EDITIONABLE TRIGGER trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "CREATE NONEDITIONABLE TRIGGER trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "  -- comment\n  CREATE OR REPLACE TRIGGER trg_test"
    ));
    assert!(QueryExecutor::is_create_trigger(
        "/* block comment */ CREATE TRIGGER trg_test"
    ));

    // Negative cases
    assert!(!QueryExecutor::is_create_trigger(
        "CREATE PROCEDURE proc_test"
    ));
    assert!(!QueryExecutor::is_create_trigger(
        "CREATE FUNCTION func_test"
    ));
    assert!(!QueryExecutor::is_create_trigger("CREATE PACKAGE pkg_test"));
    assert!(!QueryExecutor::is_create_trigger("CREATE TABLE tbl_test"));
    assert!(!QueryExecutor::is_create_trigger("SELECT * FROM dual"));
    assert!(!QueryExecutor::is_create_trigger("BEGIN :NEW := 1; END;"));
}

#[test]
fn test_compound_trigger_skips_new_old() {
    // COMPOUND TRIGGER should also skip :NEW and :OLD
    let sql = r#"CREATE OR REPLACE TRIGGER test_compound_trg
FOR UPDATE ON test_table
COMPOUND TRIGGER
  AFTER EACH ROW IS
  BEGIN
IF :NEW.status = 'ACTIVE' THEN
  INSERT INTO audit_table VALUES (:NEW.id, :audit_user, SYSDATE);
END IF;
  END AFTER EACH ROW;
END test_compound_trg;"#;
    let names = QueryExecutor::extract_bind_names(sql);
    // Only :audit_user should be extracted
    assert_eq!(
        names.len(),
        1,
        "Should have 1 bind variable, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "AUDIT_USER"),
        "Should contain AUDIT_USER, got: {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.to_uppercase() == "NEW"),
        "Should NOT contain NEW, got: {:?}",
        names
    );
}

#[test]
fn test_connect_by_not_parsed_as_tool_command() {
    // CONNECT BY는 SQL 절이므로 Tool Command로 해석되지 않아야 함
    let sql = r#"INSERT INTO oqt_nm_t (id, grp, payload)
SELECT
  oqt_nm_seq.NEXTVAL,
  'G' || TO_CHAR(MOD(level, 7)),
  TO_CLOB('seed#' || level)
FROM dual
CONNECT BY level <= 20;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("CONNECT BY"),
        "Statement should contain CONNECT BY"
    );
    assert!(
        tool_commands.is_empty(),
        "Should have no tool commands, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_start_with_single_token_path_parsed_as_start_command() {
    let items = QueryExecutor::split_script_items(
        "START with
SELECT 1 FROM dual;",
    );

    assert!(
        matches!(items.first(), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "with" && !relative_to_caller),
        "first item should parse START with as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 1 FROM dual")),
        "second item should keep trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_format_items_start_with_single_token_path_parsed_as_start_command() {
    let items = QueryExecutor::split_format_items(
        "START with
SELECT 1 FROM dual;",
    );

    assert!(
        matches!(items.first(), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "with" && !relative_to_caller),
        "first item should parse START with as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(FormatItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 1 FROM dual")),
        "second item should keep trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_start_with_not_parsed_as_tool_command() {
    let sql = r#"SELECT
  node_id,
  parent_id,
  node_name,
  LEVEL AS lvl,
  SYS_CONNECT_BY_PATH(node_name, '/') AS path
FROM oqt_t_tree
START WITH parent_id IS NULL
CONNECT BY PRIOR node_id = parent_id
ORDER SIBLINGS BY node_id;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("START WITH"),
        "Statement should contain START WITH, got: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("ORDER SIBLINGS BY"),
        "Statement should contain ORDER SIBLINGS BY, got: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "Should have no tool commands, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_print_prefix_word_not_parsed_as_print_tool_command() {
    let sql = "SELECT printable_col FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert!(
        tool_commands.is_empty(),
        "PRINT prefix in SQL identifier should not become tool command: {:?}",
        tool_commands
    );
}

#[test]
fn test_print_command_rejects_unicode_confusable_keyword() {
    let sql = "PRıNT :b_var";

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert!(
        tool_commands.is_empty(),
        "Unicode confusable keyword must not be parsed as PRINT tool command: {:?}",
        tool_commands
    );
    assert_eq!(statements, vec![sql]);
}

#[test]
fn test_prompt_prefix_word_not_parsed_as_prompt_tool_command() {
    let sql = "SELECT prompt_col FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert!(
        tool_commands.is_empty(),
        "PROMPT prefix in SQL identifier should not become tool command: {:?}",
        tool_commands
    );
}

#[test]
fn test_json_table_columns_not_parsed_as_column_tool_command() {
    let sql = r#"SELECT
  jt.order_id,
  jt.cust_name,
  jt.tier,
  it.sku,
  it.qty,
  it.price,
  (it.qty * it.price) AS line_amt
FROM oqt_t_json j
CROSS JOIN JSON_TABLE(
  j.payload,
  '$'
  COLUMNS (
    order_id   NUMBER       PATH '$.order_id',
    cust_name  VARCHAR2(50) PATH '$.customer.name',
    tier       VARCHAR2(20) PATH '$.customer.tier',
    NESTED PATH '$.items[*]'
    COLUMNS (
      sku   VARCHAR2(30) PATH '$.sku',
      qty   NUMBER       PATH '$.qty',
      price NUMBER       PATH '$.price'
    )
  )
) jt
CROSS APPLY (
  SELECT jt.sku, jt.qty, jt.price FROM dual
) it
ORDER BY jt.order_id, it.sku;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("JSON_TABLE"),
        "Statement should contain JSON_TABLE, got: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("COLUMNS ("),
        "Statement should contain COLUMNS clause, got: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "Should have no tool commands, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_match_recognize_define_not_parsed_as_tool_command() {
    let sql = r#"SELECT *
FROM oqt_t_emp
MATCH_RECOGNIZE (
  PARTITION BY deptno
  ORDER BY hiredate, empno
  MEASURES
    FIRST(ename) AS start_name,
    LAST(ename)  AS end_name,
    COUNT(*)     AS run_len
  ONE ROW PER MATCH
  PATTERN (a b+)
  DEFINE
    b AS b.sal > PREV(b.sal)
);"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("MATCH_RECOGNIZE"),
        "Statement should contain MATCH_RECOGNIZE, got: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("\n  DEFINE\n"),
        "Statement should contain DEFINE clause marker, got: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "Should have no tool commands, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_match_recognize_inline_define_not_parsed_as_tool_command() {
    let sql = r#"SELECT *
FROM oqt_t_emp
MATCH_RECOGNIZE (
  PARTITION BY deptno
  ORDER BY hiredate, empno
  PATTERN (a b+)
  DEFINE b AS b.sal > PREV(b.sal)
);"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("DEFINE b AS b.sal > PREV(b.sal)"),
        "Statement should preserve MATCH_RECOGNIZE inline DEFINE clause, got: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "Inline DEFINE in MATCH_RECOGNIZE should not be parsed as tool command, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_split_format_items_match_recognize_inline_define_not_parsed_as_tool_command() {
    let sql = r#"SELECT *
FROM oqt_t_emp
MATCH_RECOGNIZE (
  PARTITION BY deptno
  ORDER BY hiredate, empno
  PATTERN (a b+)
  DEFINE b AS b.sal > PREV(b.sal)
);"#;

    let items = QueryExecutor::split_format_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&FormatItem> = items
        .iter()
        .filter(|item| matches!(item, FormatItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "split_format_items should keep MATCH_RECOGNIZE inline DEFINE in one statement: {:?}",
        statements
    );
    assert!(
        statements[0].contains("DEFINE b AS b.sal > PREV(b.sal)"),
        "Formatted statement should preserve MATCH_RECOGNIZE inline DEFINE clause: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "split_format_items should not emit tool commands for MATCH_RECOGNIZE inline DEFINE, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_multiline_set_not_parsed_as_tool_command() {
    let sql = r#"ALTER SESSION
SET CURRENT_SCHEMA = APP_USER;
SELECT 1 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "Should split into ALTER SESSION + SELECT, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("ALTER SESSION")
            && statements[0].contains("SET CURRENT_SCHEMA = APP_USER"),
        "First statement should preserve ALTER SESSION SET clause, got: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "ALTER SESSION SET clause should not become tool command, got: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_with_inline_block_comment_header_keeps_set_clause_as_sql() {
    let sql = r#"ALTER /* session config */ SESSION
SET CURRENT_SCHEMA = APP_USER;
SELECT 2 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "ALTER SESSION with inline block comment should stay one statement before trailing SELECT: {:?}",
        statements
    );
    assert!(
        statements[0].contains("ALTER /* session config */ SESSION")
            && statements[0].contains("SET CURRENT_SCHEMA = APP_USER"),
        "first statement should keep commented ALTER SESSION header and SET clause together: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "commented ALTER SESSION SET should not be parsed as SQL*Plus SET command: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_with_inline_line_comment_header_keeps_set_clause_as_sql() {
    let sql = r#"ALTER -- keep comment
SESSION
SET CURRENT_SCHEMA = APP_USER;
SELECT 3 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "ALTER SESSION with inline line comment should stay one statement before trailing SELECT: {:?}",
        statements
    );
    assert!(
        statements[0].contains("ALTER -- keep comment")
            && statements[0].contains("SESSION")
            && statements[0].contains("SET CURRENT_SCHEMA = APP_USER"),
        "first statement should keep inline-comment ALTER SESSION header and SET clause together: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "inline-comment ALTER SESSION SET should not be parsed as SQL*Plus SET command: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_line_comment_header_with_sqlplus_like_option_keeps_set_clause_as_sql() {
    let sql = r#"ALTER -- keep header comment
SESSION
SET FLAGGER = ENTRY;
SELECT 4 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "ALTER SESSION SET FLAGGER should remain SQL even when ALTER header has inline comment: {:?}",
        statements
    );
    assert!(
        statements[0].contains("SET FLAGGER = ENTRY"),
        "first statement should preserve ALTER SESSION SET FLAGGER clause: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "ALTER SESSION SET FLAGGER must not be parsed as SQL*Plus SET command: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_block_comment_header_with_sqlplus_like_option_keeps_set_clause_as_sql() {
    let sql = r#"ALTER /* keep header comment */ SESSION
SET FLAGGER = ENTRY;
SELECT 5 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let tool_commands: Vec<&ScriptItem> = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "ALTER SESSION SET FLAGGER should remain SQL when ALTER header has inline block comment: {:?}",
        statements
    );
    assert!(
        statements[0].contains("SET FLAGGER = ENTRY"),
        "first statement should preserve ALTER SESSION SET FLAGGER clause: {}",
        statements[0]
    );
    assert!(
        tool_commands.is_empty(),
        "ALTER SESSION SET FLAGGER must not be parsed as SQL*Plus SET command: {:?}",
        tool_commands
    );
}

#[test]
fn test_alter_session_q_quote_with_semicolon_not_split() {
    let sql = r#"ALTER SESSION SET TRACEFILE_IDENTIFIER = q'[trace;session]';
SELECT 1 FROM DUAL;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        2,
        "q-quote with semicolon inside ALTER SESSION should remain one statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains(r#"q'[trace;session]'"#),
        "ALTER SESSION statement should preserve q-quoted value, got: {}",
        statements[0]
    );
}

#[test]
fn test_connect_tool_command_still_works() {
    // 실제 CONNECT Tool Command는 여전히 동작해야 함
    let sql = "CONNECT user/pass@localhost:1521/ORCL";
    let items = QueryExecutor::split_script_items(sql);

    let has_connect_command = items
        .iter()
        .any(|item| matches!(item, ScriptItem::ToolCommand(ToolCommand::Connect { .. })));
    assert!(
        has_connect_command,
        "CONNECT tool command should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_conn_tool_command_without_arguments_is_classified_as_tool_command() {
    let sql = "CONN";
    let items = QueryExecutor::split_script_items(sql);

    let has_connect_error = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Unsupported {
                message,
                is_error: true,
                ..
            }) if message.contains("CONNECT requires connection string")
        )
    });

    assert!(
        has_connect_error,
        "bare CONN should be treated as CONNECT tool command error, got: {:?}",
        items
    );
}

#[test]
fn test_connect_tool_command_supports_at_sign_in_password() {
    let sql = "CONNECT user/p@ss@localhost:1521/ORCL";
    let items = QueryExecutor::split_script_items(sql);

    let has_expected_connect = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Connect {
                username,
                password,
                host,
                port,
                service_name,
            }) if username == "user"
                && password == "p@ss"
                && host == "localhost"
                && *port == 1521
                && service_name == "ORCL"
        )
    });

    assert!(
        has_expected_connect,
        "CONNECT command with @ in password should parse correctly, got: {:?}",
        items
    );
}

#[test]
fn test_connect_tool_command_supports_slash_in_password() {
    let sql = "CONNECT user/pa/ss@localhost:1521/ORCL";
    let items = QueryExecutor::split_script_items(sql);

    let has_expected_connect = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Connect {
                username,
                password,
                host,
                port,
                service_name,
            }) if username == "user"
                && password == "pa/ss"
                && host == "localhost"
                && *port == 1521
                && service_name == "ORCL"
        )
    });

    assert!(
        has_expected_connect,
        "CONNECT command with / in password should parse correctly, got: {:?}",
        items
    );
}

#[test]
fn test_connect_tool_command_supports_tns_alias() {
    let sql = "CONNECT user/pass@LOCAL_FREE";
    let items = QueryExecutor::split_script_items(sql);

    let has_expected_connect = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Connect {
                username,
                password,
                host,
                port,
                service_name,
            }) if username == "user"
                && password == "pass"
                && host.is_empty()
                && *port == 0
                && service_name == "LOCAL_FREE"
        )
    });

    assert!(
        has_expected_connect,
        "CONNECT command with TNS alias should parse correctly, got: {:?}",
        items
    );
}

#[test]
fn test_transaction_at_command_maps_to_set_autocommit_off() {
    let command = QueryExecutor::parse_tool_command("@TRANSACTION");

    assert!(
        matches!(
            command,
            Some(ToolCommand::SetAutoCommit { enabled: false })
        ),
        "@TRANSACTION should disable autocommit instead of being treated as a script include: {command:?}"
    );
}

#[test]
fn test_set_autocommit_accepts_sql_assignment_values() {
    for (sql, expected) in [
        ("SET AUTOCOMMIT = 0", false),
        ("SET AUTOCOMMIT=1", true),
        ("SET AUTOCOMMIT TRUE", true),
        ("SET AUTOCOMMIT = FALSE", false),
    ] {
        let command = QueryExecutor::parse_tool_command(sql);
        assert!(
            matches!(command, Some(ToolCommand::SetAutoCommit { enabled }) if enabled == expected),
            "{sql} should parse as SET AUTOCOMMIT {expected}, got {command:?}"
        );
    }
}

#[test]
fn test_connect_tool_command_rejects_host_port_without_service_name() {
    let sql = "CONNECT user/pass@localhost:1521";
    let items = QueryExecutor::split_script_items(sql);

    let has_expected_error = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Unsupported {
                message,
                is_error: true,
                ..
            }) if message.contains("Invalid connection string")
        )
    });

    assert!(
        has_expected_error,
        "CONNECT command missing service name should remain a syntax error, got: {:?}",
        items
    );
}

#[test]
fn test_column_new_value_tool_command_parsed() {
    let sql = "COLUMN col NEW_VALUE var";
    let items = QueryExecutor::split_script_items(sql);

    let has_column_command = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::ColumnNewValue {
                column_name,
                variable_name
            }) if column_name == "col" && variable_name == "var"
        )
    });
    assert!(
        has_column_command,
        "COLUMN NEW_VALUE tool command should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_column_report_layout_command_is_parsed() {
    let sql = "COLUMN col HEADING test";
    let items = QueryExecutor::split_script_items(sql);

    let has_column_layout = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SqlPlusReportLayout { raw })
                if raw.eq_ignore_ascii_case("COLUMN col HEADING test")
        )
    });
    assert!(
        has_column_layout,
        "COLUMN report-layout command should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_set_trimspool_command_parsed() {
    let sql = "SET TRIMSPOOL ON";
    let items = QueryExecutor::split_script_items(sql);

    let has_trimspool = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetTrimSpool { enabled: true })
        )
    });
    assert!(
        has_trimspool,
        "SET TRIMSPOOL should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_set_trimout_command_parsed() {
    let sql = "SET TRIMOUT OFF";
    let items = QueryExecutor::split_script_items(sql);

    let has_trimout = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetTrimOut { enabled: false })
        )
    });
    assert!(
        has_trimout,
        "SET TRIMOUT should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_set_sqlblanklines_command_parsed() {
    let sql = "SET SQLBLANKLINES ON";
    let items = QueryExecutor::split_script_items(sql);

    let has_sqlblanklines = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetSqlBlankLines { enabled: true })
        )
    });
    assert!(
        has_sqlblanklines,
        "SET SQLBLANKLINES should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_split_script_items_multiline_set_serveroutput_command() {
    let sql = "SET SERVEROUTPUT\n    ON SIZE UNLIMITED;\nSELECT 1 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(
            &items[0],
            ScriptItem::ToolCommand(ToolCommand::SetServerOutput {
                enabled: true,
                size: None,
                unlimited: true
            })
        ),
        "multiline SET SERVEROUTPUT should remain one tool command, got: {:?}",
        items
    );
    assert!(
        matches!(&items[1], ScriptItem::Statement(stmt) if stmt.eq_ignore_ascii_case("SELECT 1 FROM dual")),
        "SELECT following multiline SET SERVEROUTPUT should stay a SQL statement, got: {:?}",
        items
    );
}

#[test]
fn test_split_format_items_multiline_set_serveroutput_command() {
    let sql = "SET SERVEROUTPUT\n    ON SIZE UNLIMITED;\nSELECT 1 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);

    assert!(
        matches!(
            &items[0],
            FormatItem::ToolCommand(ToolCommand::SetServerOutput {
                enabled: true,
                size: None,
                unlimited: true
            })
        ),
        "format splitter should keep multiline SET SERVEROUTPUT as one tool command, got: {:?}",
        items
    );
    assert!(
        matches!(&items[1], FormatItem::Statement(stmt) if stmt.eq_ignore_ascii_case("SELECT 1 FROM dual")),
        "format splitter should keep trailing SELECT separate after multiline SET command, got: {:?}",
        items
    );
}

#[test]
fn test_sqlblanklines_off_splits_top_level_statement_on_blank_line() {
    let sql = "SET SQLBLANKLINES OFF\nSELECT 1\n\nFROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert_eq!(
        items.len(),
        3,
        "Blank line should split statement when SQLBLANKLINES is OFF"
    );
    assert!(matches!(
        &items[0],
        ScriptItem::ToolCommand(ToolCommand::SetSqlBlankLines { enabled: false })
    ));
    assert!(
        matches!(&items[1], ScriptItem::Statement(stmt) if stmt.eq_ignore_ascii_case("SELECT 1"))
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.eq_ignore_ascii_case("FROM dual"))
    );
}

#[test]
fn test_default_sqlblanklines_on_keeps_blank_line_inside_statement() {
    let sql = "SELECT *\n\nFROM user_tables;";
    let items = QueryExecutor::split_script_items(sql);

    assert_eq!(
        items.len(),
        1,
        "Blank line should NOT split statement by default (SQLBLANKLINES ON)"
    );
    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("SELECT") && stmt.contains("FROM"))
    );
}

#[test]
fn test_sqlblanklines_on_keeps_blank_line_inside_top_level_statement() {
    let sql = "SET SQLBLANKLINES ON\nSELECT 1\n\nFROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert_eq!(
        items.len(),
        2,
        "Expected SET command and one SELECT statement"
    );
    assert!(matches!(
        items[0],
        ScriptItem::ToolCommand(ToolCommand::SetSqlBlankLines { enabled: true })
    ));
    assert!(matches!(
        &items[1],
        ScriptItem::Statement(stmt) if stmt.contains("SELECT 1\n\nFROM dual")
    ));
}

#[test]
fn test_set_tab_command_parsed() {
    let sql = "SET TAB OFF";
    let items = QueryExecutor::split_script_items(sql);

    let has_tab = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetTab { enabled: false })
        )
    });
    assert!(has_tab, "SET TAB should be recognized, got: {:?}", items);
}

#[test]
fn test_set_define_single_quoted_char_parsed() {
    let sql = "SET DEFINE '^'";
    let items = QueryExecutor::split_script_items(sql);

    let has_set_define = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetDefine {
                enabled: true,
                define_char: Some('^')
            })
        )
    });
    assert!(
        has_set_define,
        "SET DEFINE '^' should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_set_define_single_quote_only_does_not_panic() {
    let sql = "SET DEFINE '";
    let items = QueryExecutor::split_script_items(sql);

    let has_quoted_define_char = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetDefine {
                enabled: true,
                define_char: Some('\'')
            })
        )
    });
    assert!(
        has_quoted_define_char,
        "SET DEFINE with single quote should be handled safely, got: {:?}",
        items
    );
}

#[test]
fn test_set_colsep_command_parsed() {
    let sql = "SET COLSEP ||";
    let items = QueryExecutor::split_script_items(sql);

    let has_colsep = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetColSep { separator }) if separator == "||"
        )
    });
    assert!(
        has_colsep,
        "SET COLSEP should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_set_null_command_parsed() {
    let sql = "SET NULL (null)";
    let items = QueryExecutor::split_script_items(sql);

    let has_set_null = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::SetNull { null_text }) if null_text == "(null)"
        )
    });
    assert!(
        has_set_null,
        "SET NULL should be recognized, got: {:?}",
        items
    );
}

#[test]
fn sqlplus_quoted_empty_and_whitespace_text_arguments_round_trip_semantically() {
    let sql = "SET NULL ''\nSET COLSEP ' '\nSET NULL 'can''t render'";
    let items = QueryExecutor::split_script_items(sql);

    assert!(matches!(
        items.first(),
        Some(ScriptItem::ToolCommand(ToolCommand::SetNull { null_text }))
            if null_text.is_empty()
    ));
    assert!(matches!(
        items.get(1),
        Some(ScriptItem::ToolCommand(ToolCommand::SetColSep { separator }))
            if separator == " "
    ));
    assert!(matches!(
        items.get(2),
        Some(ScriptItem::ToolCommand(ToolCommand::SetNull { null_text }))
            if null_text == "can't render"
    ));
}

#[test]
fn test_spool_file_command_parsed() {
    let sql = "SPOOL output.log";
    let items = QueryExecutor::split_script_items(sql);

    let has_spool_file = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Spool { path: Some(path), append: false })
                if path == "output.log"
        )
    });
    assert!(
        has_spool_file,
        "SPOOL file should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_spool_append_command_parsed() {
    let sql = "SPOOL APPEND";
    let items = QueryExecutor::split_script_items(sql);

    let has_spool_append = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Spool {
                path: None,
                append: true
            })
        )
    });
    assert!(
        has_spool_append,
        "SPOOL APPEND should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_spool_off_command_parsed() {
    let sql = "SPOOL OFF";
    let items = QueryExecutor::split_script_items(sql);

    let has_spool_off = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Spool {
                path: None,
                append: false
            })
        )
    });
    assert!(
        has_spool_off,
        "SPOOL OFF should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_break_on_command_parsed() {
    let sql = "BREAK ON deptno";
    let items = QueryExecutor::split_script_items(sql);

    let has_break_on = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::BreakOn { column_name }) if column_name == "deptno"
        )
    });
    assert!(
        has_break_on,
        "BREAK ON should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_break_off_command_parsed() {
    let sql = "BREAK OFF";
    let items = QueryExecutor::split_script_items(sql);

    let has_break_off = items
        .iter()
        .any(|item| matches!(item, ScriptItem::ToolCommand(ToolCommand::BreakOff)));
    assert!(
        has_break_off,
        "BREAK OFF should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_compute_sum_command_parsed() {
    let sql = "COMPUTE SUM";
    let items = QueryExecutor::split_script_items(sql);

    let has_compute_sum = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Compute {
                mode: crate::db::ComputeMode::Sum,
                label: None,
                of_column: None,
                on_column: None
            })
        )
    });
    assert!(
        has_compute_sum,
        "COMPUTE SUM should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_compute_count_command_parsed() {
    let sql = "COMPUTE COUNT";
    let items = QueryExecutor::split_script_items(sql);

    let has_compute_count = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Compute {
                mode: crate::db::ComputeMode::Count,
                label: None,
                of_column: None,
                on_column: None
            })
        )
    });
    assert!(
        has_compute_count,
        "COMPUTE COUNT should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_compute_off_command_parsed() {
    let sql = "COMPUTE OFF";
    let items = QueryExecutor::split_script_items(sql);

    let has_compute_off = items
        .iter()
        .any(|item| matches!(item, ScriptItem::ToolCommand(ToolCommand::ComputeOff)));
    assert!(
        has_compute_off,
        "COMPUTE OFF should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_compute_count_of_on_command_parsed() {
    let sql = "COMPUTE COUNT OF id ON grp";
    let items = QueryExecutor::split_script_items(sql);

    let has_compute_count_of_on = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Compute {
                mode: crate::db::ComputeMode::Count,
                label: None,
                of_column: Some(of_col),
                on_column: Some(on_col)
            }) if of_col == "id" && on_col == "grp"
        )
    });
    assert!(
        has_compute_count_of_on,
        "COMPUTE COUNT OF ... ON ... should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_compute_sum_of_on_command_parsed() {
    let sql = "COMPUTE SUM OF val ON grp";
    let items = QueryExecutor::split_script_items(sql);

    let has_compute_sum_of_on = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Compute {
                mode: crate::db::ComputeMode::Sum,
                label: None,
                of_column: Some(of_col),
                on_column: Some(on_col)
            }) if of_col == "val" && on_col == "grp"
        )
    });
    assert!(
        has_compute_sum_of_on,
        "COMPUTE SUM OF ... ON ... should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_clear_breaks_computes_parsed() {
    let sql = "CLEAR BREAKS CLEAR COMPUTES";
    let items = QueryExecutor::split_script_items(sql);
    let has_clear_both = items.iter().any(|item| {
        matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::ClearBreaksComputes)
        )
    });
    assert!(
        has_clear_both,
        "CLEAR BREAKS CLEAR COMPUTES should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_clear_breaks_parsed() {
    let sql = "CLEAR BREAKS";
    let items = QueryExecutor::split_script_items(sql);
    let has_clear_breaks = items
        .iter()
        .any(|item| matches!(item, ScriptItem::ToolCommand(ToolCommand::ClearBreaks)));
    assert!(
        has_clear_breaks,
        "CLEAR BREAKS should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_clear_computes_parsed() {
    let sql = "CLEAR COMPUTES";
    let items = QueryExecutor::split_script_items(sql);
    let has_clear_computes = items
        .iter()
        .any(|item| matches!(item, ScriptItem::ToolCommand(ToolCommand::ClearComputes)));
    assert!(
        has_clear_computes,
        "CLEAR COMPUTES should be recognized, got: {:?}",
        items
    );
}

#[test]
fn test_wave12_sqlplus_substitution_and_report_commands_are_supported() {
    for (sql, expected) in [
        ("SET CONCAT #", "concat"),
        ("SET CONCAT .", "concat"),
        ("SET ESCAPE \\", "escape"),
        ("SET ESCAPE OFF", "escape"),
        ("SET HEADSEP |", "report layout"),
        (
            "COLUMN w12_wrapped FORMAT A12 HEADING 'left|right' WRAP",
            "report layout",
        ),
        (
            "COLUMN w12_amount FORMAT 999,990.00 JUSTIFY LEFT",
            "report layout",
        ),
        ("CLEAR COLUMNS", "report layout"),
    ] {
        let parsed = QueryExecutor::parse_tool_command(sql);
        let supported = matches!(
            (&parsed, expected),
            (Some(ToolCommand::SetConcat { .. }), "concat")
                | (Some(ToolCommand::SetEscape { .. }), "escape")
                | (
                    Some(ToolCommand::SqlPlusReportLayout { .. }),
                    "report layout"
                )
        );
        assert!(supported, "{sql} should be supported, got {parsed:?}");
    }
}

#[test]
fn test_accept_prompt_with_utf8_prefix_before_prompt_keyword() {
    let sql = "ACCEPT v ıprompt '메시지'";
    let items = QueryExecutor::split_script_items(sql);

    let parsed = items.iter().find_map(|item| match item {
        ScriptItem::ToolCommand(ToolCommand::Accept { name, prompt }) => {
            Some((name.as_str(), prompt.as_deref()))
        }
        _ => None,
    });

    assert_eq!(
        parsed,
        Some(("v", Some("메시지"))),
        "UTF-8 text before PROMPT should not break prompt slicing: {:?}",
        items
    );
}

#[test]
fn test_prompt_command_preserves_trailing_semicolon_text() {
    let sql = "PROMPT hello;";
    let items = QueryExecutor::split_script_items(sql);

    let parsed = items.iter().find_map(|item| match item {
        ScriptItem::ToolCommand(ToolCommand::Prompt { text }) => Some(text.as_str()),
        _ => None,
    });

    assert_eq!(
        parsed,
        Some("hello;"),
        "PROMPT payload should preserve trailing semicolon text: {:?}",
        items
    );
}

#[test]
fn test_rem_command_consumes_the_entire_physical_line_with_semicolons() {
    let sql =
        "REM the next statement is one line; this REM ends with a semicolon;\nSELECT 1 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements,
        vec!["SELECT 1 FROM dual"],
        "REM payload semicolons must not be split into executable SQL: {items:?}"
    );
}

#[test]
fn test_labeled_anonymous_block_keeps_named_end_semicolon_for_execute() {
    let sql = r#"<<outer_label>>
DECLARE
  value NUMBER := 1;
BEGIN
  value := value + 1;
END outer_label;
/"#;
    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        1,
        "labeled anonymous block must remain one statement: {items:?}"
    );
    let normalized = QueryExecutor::normalize_sql_for_execute(statements[0]);
    assert!(
        normalized.ends_with("END outer_label;"),
        "OCI execution must retain the mandatory named-END semicolon: {normalized:?}"
    );
}

#[test]
fn test_trigger_with_declare_and_multiline_header() {
    // TRIGGER 헤더에서 이벤트 타입(INSERT)이 별도 행에 있고,
    // DECLARE 블록과 q-quote 내의 가짜 키워드가 포함된 경우
    let sql = r#"CREATE OR REPLACE TRIGGER oqt_nm_trg BEFORE
INSERT
ON oqt_nm_t
FOR EACH ROW
DECLARE
v VARCHAR2 (2000);
BEGIN
v := q '[TRG: fake tokens END; / ; BEGIN CASE LOOP IF THEN ELSE]' || ' + '' ; ''';
:new.payload := NVL (:new.payload, TO_CLOB ('')) || CHR (10) || v;
END;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(statements[0].contains("CREATE OR REPLACE TRIGGER oqt_nm_trg"));
    assert!(statements[0].contains("DECLARE"));
    assert!(statements[0].contains("END"));
}

#[test]
fn test_nq_quote_string_parsing() {
    // Test nq'[...]' (National Character q-quoted string) parsing
    let sql = r#"SELECT nq'[한글 문자열]' FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("nq'[한글 문자열]'"),
        "Statement should contain nq'[...]', got: {}",
        statements[0]
    );
}

#[test]
fn test_nq_quote_with_semicolon_inside() {
    // Test that semicolons inside nq'...' don't split the statement
    let sql = r#"SELECT nq'[text with ; semicolon]' FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("nq'[text with ; semicolon]'"),
        "Statement should preserve semicolon inside nq'...', got: {}",
        statements[0]
    );
}

#[test]
fn test_split_script_items_dollar_quote_keeps_begin_end_inside_string() {
    let sql = r#"SELECT $$BEGIN END$$ AS text FROM dual;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "dollar-quote content should not affect block depth or statement split: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("$$BEGIN END$$"),
        "first statement should preserve dollar-quoted text, got: {}",
        stmts[0]
    );
    assert!(
        stmts[1].contains("SELECT 2 FROM dual"),
        "second statement should remain independent, got: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_tagged_dollar_quote_ignores_semicolon_and_parenthesis() {
    let sql = r#"SELECT $tag$foo(bar); END IF;$tag$ AS text FROM dual;
SELECT 3 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "tagged dollar-quote should ignore internal semicolon/paren keywords: {:?}",
        stmts
    );
    assert!(
        stmts[0].contains("$tag$foo(bar); END IF;$tag$"),
        "first statement should keep tagged dollar-quote intact, got: {}",
        stmts[0]
    );
    assert!(
        stmts[1].contains("SELECT 3 FROM dual"),
        "second statement should be split at top-level semicolon, got: {}",
        stmts[1]
    );
}

#[test]
fn test_nq_quote_different_delimiters() {
    // Test nq'...' with different delimiters: (), {}, <>
    let sql1 = r#"SELECT nq'(parentheses)' FROM dual"#;
    let sql2 = r#"SELECT nq'{braces}' FROM dual"#;
    let sql3 = r#"SELECT nq'<angle brackets>' FROM dual"#;
    let sql4 = r#"SELECT Nq'!custom delimiter!' FROM dual"#;

    let items1 = QueryExecutor::split_script_items(sql1);
    let items2 = QueryExecutor::split_script_items(sql2);
    let items3 = QueryExecutor::split_script_items(sql3);
    let items4 = QueryExecutor::split_script_items(sql4);

    assert_eq!(items1.len(), 1, "nq'(...)' should parse as 1 statement");
    assert_eq!(items2.len(), 1, "nq'{{...}}' should parse as 1 statement");
    assert_eq!(items3.len(), 1, "nq'<...>' should parse as 1 statement");
    assert_eq!(items4.len(), 1, "Nq'!...!' should parse as 1 statement");
}

#[test]
fn test_nq_quote_case_insensitive() {
    // Test that NQ, Nq, nQ, nq all work
    let sql1 = r#"SELECT nq'[lower]' FROM dual"#;
    let sql2 = r#"SELECT NQ'[upper]' FROM dual"#;
    let sql3 = r#"SELECT Nq'[mixed1]' FROM dual"#;
    let sql4 = r#"SELECT nQ'[mixed2]' FROM dual"#;

    let items1 = QueryExecutor::split_script_items(sql1);
    let items2 = QueryExecutor::split_script_items(sql2);
    let items3 = QueryExecutor::split_script_items(sql3);
    let items4 = QueryExecutor::split_script_items(sql4);

    assert_eq!(items1.len(), 1, "nq'...' should parse correctly");
    assert_eq!(items2.len(), 1, "NQ'...' should parse correctly");
    assert_eq!(items3.len(), 1, "Nq'...' should parse correctly");
    assert_eq!(items4.len(), 1, "nQ'...' should parse correctly");
}

#[test]
fn test_nq_quote_in_plsql_block() {
    // Test nq'...' inside PL/SQL block
    let sql = r#"DECLARE
v_text VARCHAR2(100);
BEGIN
v_text := nq'[Hello; World; End;]';
DBMS_OUTPUT.PUT_LINE(v_text);
END;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 PL/SQL block, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("nq'[Hello; World; End;]'"),
        "PL/SQL block should contain nq'...' string intact"
    );
}

#[test]
fn test_nq_quote_mixed_with_q_quote() {
    // Test both nq'...' and q'...' in same statement
    let sql = r#"SELECT q'[regular q-quote]', nq'[national q-quote]' FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "Should be 1 statement with both q'...' and nq'...'"
    );
    assert!(statements[0].contains("q'[regular q-quote]'"));
    assert!(statements[0].contains("nq'[national q-quote]'"));
}

#[test]
fn test_nq_quote_bind_variable_extraction() {
    // Test that bind variables inside nq'...' are NOT extracted
    let sql = r#"SELECT nq'[:not_a_bind]', :real_bind FROM dual"#;
    let names = QueryExecutor::extract_bind_names(sql);

    assert_eq!(
        names.len(),
        1,
        "Should have 1 bind variable, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "REAL_BIND"),
        "Should contain REAL_BIND, got: {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.to_uppercase() == "NOT_A_BIND"),
        "Should NOT contain NOT_A_BIND (inside nq'...'), got: {:?}",
        names
    );
}

#[test]
fn test_hint_in_select_statement() {
    // Test that hints are preserved in statements
    let sql = "SELECT /*+ FULL(t) PARALLEL(t,4) */ * FROM table t;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(
        statements[0].contains("/*+ FULL(t) PARALLEL(t,4) */"),
        "Hint should be preserved in statement, got: {}",
        statements[0]
    );
}

#[test]
fn test_hint_not_split_statement() {
    // Hint should not cause statement splitting
    let sql = "SELECT /*+ INDEX(t idx1) */ col1, col2 FROM table t WHERE id = 1;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement with hint");
    assert!(statements[0].contains("/*+"));
    assert!(statements[0].contains("*/"));
}

#[test]
fn test_date_literal_parsing() {
    // DATE literals should be parsed correctly
    let sql = "SELECT DATE '2024-01-01' FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(
        statements[0].contains("DATE '2024-01-01'"),
        "DATE literal should be preserved"
    );
}

#[test]
fn test_timestamp_literal_parsing() {
    // TIMESTAMP literals should be parsed correctly
    let sql = "SELECT TIMESTAMP '2024-01-01 12:30:00' FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(
        statements[0].contains("TIMESTAMP '2024-01-01 12:30:00'"),
        "TIMESTAMP literal should be preserved"
    );
}

#[test]
fn test_interval_literal_parsing() {
    // INTERVAL literals should be parsed correctly
    let sql = "SELECT INTERVAL '5' DAY FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(
        statements[0].contains("INTERVAL '5' DAY"),
        "INTERVAL literal should be preserved"
    );
}

#[test]
fn test_interval_year_to_month_literal() {
    // INTERVAL YEAR TO MONTH literals
    let sql = "SELECT INTERVAL '1-6' YEAR TO MONTH FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("INTERVAL '1-6' YEAR TO MONTH"));
}

#[test]
fn test_multiple_datetime_literals() {
    // Multiple datetime literals in one statement
    let sql =
        "SELECT DATE '2024-01-01', TIMESTAMP '2024-01-01 12:00:00', INTERVAL '1' DAY FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("DATE '2024-01-01'"));
    assert!(statements[0].contains("TIMESTAMP '2024-01-01 12:00:00'"));
    assert!(statements[0].contains("INTERVAL '1' DAY"));
}

#[test]
fn test_flashback_query_parsing() {
    // FLASHBACK query with AS OF should parse correctly
    let sql = "SELECT * FROM employees AS OF TIMESTAMP (SYSDATE - 1/24);";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("AS OF TIMESTAMP"));
}

#[test]
fn test_fetch_first_rows_parsing() {
    // Oracle 12c+ FETCH FIRST clause
    let sql = "SELECT * FROM employees ORDER BY salary DESC FETCH FIRST 10 ROWS ONLY;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("FETCH FIRST 10 ROWS ONLY"));
}

#[test]
fn test_offset_fetch_parsing() {
    // OFFSET with FETCH
    let sql = "SELECT * FROM employees ORDER BY id OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("OFFSET 10 ROWS"));
    assert!(statements[0].contains("FETCH NEXT 5 ROWS ONLY"));
}

#[test]
fn test_listagg_within_group() {
    // LISTAGG with WITHIN GROUP
    let sql = "SELECT department_id, LISTAGG(employee_name, ', ') WITHIN GROUP (ORDER BY employee_name) AS employees FROM emp GROUP BY department_id;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("LISTAGG"));
    assert!(statements[0].contains("WITHIN GROUP"));
}

#[test]
fn test_keep_dense_rank() {
    // KEEP (DENSE_RANK FIRST/LAST ORDER BY)
    let sql = "SELECT department_id, MAX(salary) KEEP (DENSE_RANK FIRST ORDER BY hire_date) AS first_salary FROM employees GROUP BY department_id;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("KEEP (DENSE_RANK FIRST ORDER BY hire_date)"));
}

#[test]
fn test_pivot_query() {
    // PIVOT query
    let sql = r#"SELECT * FROM sales_data
PIVOT (
SUM(amount)
FOR month IN ('JAN', 'FEB', 'MAR')
);"#;
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("PIVOT"));
    assert!(statements[0].contains("SUM(amount)"));
}

#[test]
fn test_sample_query() {
    // SAMPLE clause
    let sql = "SELECT * FROM large_table SAMPLE (10) SEED (42);";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("SAMPLE (10)"));
    assert!(statements[0].contains("SEED (42)"));
}

#[test]
fn test_for_update_skip_locked() {
    // FOR UPDATE with SKIP LOCKED
    let sql = "SELECT * FROM jobs WHERE status = 'PENDING' FOR UPDATE SKIP LOCKED;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("FOR UPDATE SKIP LOCKED"));
}

#[test]
fn test_analytic_window_frame() {
    // Analytic function with ROWS BETWEEN
    let sql = "SELECT employee_id, salary, SUM(salary) OVER (ORDER BY hire_date ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) running_total FROM employees;";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 1, "Should be 1 statement");
    assert!(statements[0].contains("ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW"));
}

#[test]
fn test_type_body_with_q_quoted_string() {
    // TYPE BODY with q-quoted string containing special characters
    // The q'[...]' syntax allows embedding ; / -- /* */ without escaping
    let sql = r#"CREATE OR REPLACE TYPE BODY oqt_obj AS
  MEMBER FUNCTION peek RETURN VARCHAR2 IS
  BEGIN
RETURN 'peek:'||SUBSTR(txt,1,40)||q'[ | tokens: END; / ; /* */ -- ]';
  END;
END;
/
SHOW ERRORS TYPE BODY oqt_obj"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    // Should have exactly 1 statement (the TYPE BODY)
    // SHOW ERRORS is a tool command, not a statement
    assert_eq!(
        stmts.len(),
        1,
        "Should have 1 statement (TYPE BODY), got {} statements: {:?}",
        stmts.len(),
        stmts
    );

    // The statement should contain the full TYPE BODY
    assert!(
        stmts[0].contains("CREATE OR REPLACE TYPE BODY oqt_obj"),
        "Should contain CREATE OR REPLACE TYPE BODY"
    );
    assert!(
        stmts[0].contains("MEMBER FUNCTION peek"),
        "Should contain MEMBER FUNCTION"
    );
    assert!(
        stmts[0].contains(r#"q'[ | tokens: END; / ; /* */ -- ]'"#),
        "Should contain q-quoted string intact"
    );
    assert!(
        stmts[0].ends_with("END") || stmts[0].ends_with("END;"),
        "Should end with END or END;, got: {}",
        &stmts[0][stmts[0].len().saturating_sub(50)..]
    );

    // Verify SHOW ERRORS is parsed as tool command
    let tool_commands: Vec<&ToolCommand> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::ToolCommand(cmd) => Some(cmd),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_commands.len(),
        1,
        "Should have 1 tool command (SHOW ERRORS)"
    );
}

#[test]
fn test_package_body_with_comments_does_not_break_depth() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY oqt_comment_pkg AS
  /* package-level comment with keywords: BEGIN END IF LOOP */
  PROCEDURE p_test (p_id NUMBER) IS
    /* procedure comment */
  BEGIN
    /* begin-block comment */
    NULL;
  END p_test;

  -- another comment mentioning END;
  PROCEDURE p_test2 IS
  BEGIN
    NULL;
  END p_test2;
END oqt_comment_pkg;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        2,
        "Comments should not affect depth/splitting; expected package body + select, got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("CREATE OR REPLACE PACKAGE BODY oqt_comment_pkg"),
        "First statement should be package body"
    );
    assert!(
        statements[0].contains("END oqt_comment_pkg"),
        "Package body should end correctly"
    );
    assert!(
        statements[1].contains("SELECT 1 FROM dual"),
        "Second statement should be trailing SELECT"
    );
}

#[test]
fn test_line_block_depths_package_body_with_nested_end_labels_and_exception() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY test_pkg AS
  FUNCTION f1 RETURN NUMBER IS
    v NUMBER := 0;
  BEGIN
    IF v = 0 THEN
      v := 1;
    ELSE
      v := 2;
    END IF;
    RETURN v;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN -1;
  END f1;
BEGIN
  NULL;
END test_pkg;"#;

    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(depths.len(), 17, "unexpected depth vector: {depths:?}");
    assert!(
        depths[5] > depths[4],
        "IF body should indent (depths: {depths:?})"
    );
    assert_eq!(
        depths[6], depths[4],
        "ELSE should pre-dedent to IF depth (depths: {depths:?})"
    );
    assert_eq!(
        depths[8], depths[4],
        "END IF should return to IF depth (depths: {depths:?})"
    );
    assert!(
        depths[11] > depths[10],
        "EXCEPTION handler WHEN should indent under EXCEPTION (depths: {depths:?})"
    );
    assert_eq!(
        depths[16], 0,
        "named package-body END should close both BEGIN and AS/IS scopes (depths: {depths:?})"
    );
}

#[test]
fn test_line_block_depths_package_body_with_subprogram_end_name_on_next_line() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY test_pkg AS
  PROCEDURE p IS
  BEGIN
    NULL;
  END
  p;
END test_pkg;"#;

    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(depths.len(), 7, "unexpected depth vector: {depths:?}");
    assert_eq!(
        depths[4], depths[5],
        "split END / name suffix should keep depth stable (depths: {depths:?})"
    );
    assert_eq!(
        depths[6], 0,
        "final END <package_name> should fully close package-body scope (depths: {depths:?})"
    );
}

#[test]
fn test_line_block_depths_package_body_with_schema_qualified_end_name_and_nested_blocks() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY test_schema.test_pkg AS
  PROCEDURE p_run(p_flag IN NUMBER) IS
  BEGIN
    IF p_flag = 1 THEN
      NULL;
    ELSIF p_flag = 2 THEN
      NULL;
    ELSE
      NULL;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END p_run;
BEGIN
  NULL;
END test_schema.test_pkg;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    assert_eq!(depths.len(), 18, "unexpected depth vector: {depths:?}");
    assert_eq!(
        depths[16],
        0,
        "schema-qualified package END line should already be at top-level depth (depths: {depths:?})"
    );

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);
    assert_eq!(
        statements.len(),
        2,
        "expected package body + trailing select; got: {:?}",
        statements
    );
    assert!(
        statements[0].contains("END test_schema.test_pkg"),
        "first statement should include schema-qualified package END"
    );
    assert!(
        statements[1].contains("SELECT 1 FROM dual"),
        "second statement should be trailing select"
    );
}

#[test]
fn test_line_block_depths_package_body_keeps_pending_end_for_quoted_split_labels() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY "TEST_PKG" AS
  FUNCTION f1 RETURN NUMBER IS
  BEGIN
    IF 1 = 1 THEN
      RETURN 1;
    ELSE
      RETURN 2;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN -1;
  END
  "f1";
BEGIN
  NULL;
END
"TEST_PKG";"#;

    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(depths.len(), 17, "unexpected depth vector: {depths:?}");
    assert_eq!(
        depths[12], depths[11],
        "split quoted subprogram END label should keep depth stable (depths: {depths:?})"
    );
    assert_eq!(
        depths[16], 0,
        "split quoted package END label should fully close package body scope (depths: {depths:?})"
    );
}

#[test]
fn test_split_script_items_package_body_with_split_quoted_end_labels() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY "TEST_PKG" AS
  FUNCTION f1 RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END
  "f1";
BEGIN
  NULL;
END
"TEST_PKG";
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        2,
        "split quoted END labels should keep package body and trailing SELECT separated: {:?}",
        statements
    );
    assert!(
        statements[0].contains("END\n  \"f1\";"),
        "first statement should preserve split quoted subprogram END label"
    );
    assert!(
        statements[0].contains("END") && statements[0].contains("\"TEST_PKG\""),
        "first statement should preserve split quoted package END label: {:?}",
        statements[0]
    );
    assert!(
        statements[1].contains("SELECT 1 FROM dual"),
        "second statement should contain trailing SELECT"
    );
}

#[test]
fn test_split_script_items_package_body_with_quoted_schema_qualified_end_name() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY "TEST_SCHEMA"."TEST_PKG" AS
  PROCEDURE p IS
  BEGIN
    NULL;
  END p;
BEGIN
  NULL;
END "TEST_SCHEMA"."TEST_PKG";
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);
    assert_eq!(
        statements.len(),
        2,
        "quoted schema-qualified END should not swallow the trailing statement: {:?}",
        statements
    );
    assert!(
        statements[0].contains("END \"TEST_SCHEMA\".\"TEST_PKG\""),
        "first statement should keep quoted schema-qualified END"
    );
    assert!(
        statements[1].contains("SELECT 1 FROM dual"),
        "second statement should be trailing select"
    );
}

#[test]
fn test_line_block_depths_increase_for_if_and_case() {
    let sql = r#"BEGIN
IF v_flag = 'Y' THEN
CASE
WHEN v_num = 1 THEN
NULL;
ELSE
NULL;
END CASE;
END IF;
END;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 3, 4, 3, 4, 2, 1, 0];

    assert_eq!(depths, expected, "IF/CASE depth tracking mismatch");
}

#[test]
fn test_line_block_depths_if_with_begin_and_multiple_case_columns() {
    let sql = r#"if 1=1 then
    begin
        select
            case
                when 1=1 then '1'
                else ''
            end,
            case
                when 1=1 then '1'
                else ''
            end
        from dual;
end
end if;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 2, 3, 3, 2, 2, 3, 3, 2, 2, 1, 0];

    assert_eq!(
        depths, expected,
        "IF/BEGIN + multi-CASE depth tracking mismatch"
    );
}

#[test]
fn test_line_block_depths_mysql_elseif_is_pre_dedented() {
    let sql = r#"IF score >= 90 THEN
  SET grade = 'A';
ELSEIF score >= 80 THEN
  SET grade = 'B';
ELSE
  SET grade = 'C';
END IF;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 0, 1, 0, 1, 0];

    assert_eq!(
        depths, expected,
        "ELSEIF depth tracking should match ELSE/ELSIF pre-dedent semantics"
    );
}

#[test]
fn test_split_script_items_mysql_elseif_block_remains_single_statement() {
    let sql = r#"IF score >= 90 THEN
  SET grade = 'A';
ELSEIF score >= 80 THEN
  SET grade = 'B';
ELSE
  SET grade = 'C';
END IF;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        1,
        "IF/ELSEIF/ELSE/END IF block should be a single statement"
    );
}

#[test]
fn test_split_script_items_mysql_delimiter_keeps_routine_body_intact() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE demo_proc()
BEGIN
  SELECT 1;
  SELECT 2;
END$$
DELIMITER ;
SELECT 3;
"#;

    let items = QueryExecutor::split_script_items(sql);
    assert!(matches!(
        items.first(),
        Some(crate::db::ScriptItem::ToolCommand(crate::db::ToolCommand::MysqlDelimiter {
            delimiter
        })) if delimiter == "$$"
    ));
    assert!(matches!(
        items.get(2),
        Some(crate::db::ScriptItem::ToolCommand(crate::db::ToolCommand::MysqlDelimiter {
            delimiter
        })) if delimiter == ";"
    ));

    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "delimiter script should yield two statements: {stmts:?}"
    );
    assert!(
        stmts[0].contains("SELECT 1;") && stmts[0].contains("SELECT 2;"),
        "procedure body should keep internal semicolons: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_statement_bounds_at_cursor_mysql_delimiter_keeps_routine_body_intact() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE demo_proc()
BEGIN
  SELECT 1;
  SELECT 2;
END$$
DELIMITER ;
SELECT 3;
"#;

    let cursor = sql.find("SELECT 2").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected CREATE PROCEDURE bounds inside MySQL custom-delimiter routine");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE PROCEDURE demo_proc()"),
        "cursor inside MySQL routine should resolve the full routine statement, got:\n{statement}"
    );
    assert!(
        statement.contains("SELECT 1;") && statement.contains("SELECT 2;"),
        "routine statement bounds should keep inner semicolon-terminated statements, got:\n{statement}"
    );
    assert!(
        statement.trim_end().ends_with("END"),
        "routine statement bounds should stop at the stripped custom delimiter terminator, got:\n{statement}"
    );
    assert!(
        !statement.contains("SELECT 3"),
        "next statement must not leak into MySQL routine bounds"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_mixed_custom_delimiter_keeps_current_routine_isolated() {
    let sql = r#"DELIMITER $$
CREATE TRIGGER bi_task_log
    BEFORE INSERT
    ON task_log
    FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL
            OR NEW.hours <= 0
            OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
END$$

CREATE FUNCTION fn_efficiency_band (
    p_hours DECIMAL (12, 2),
    p_budget DECIMAL (14, 2)
) RETURNS VARCHAR (16) DETERMINISTIC
BEGIN
    DECLARE v_score DECIMAL (12, 4);
    SET v_score = IFNULL (p_hours / NULLIF (p_budget / 1000, 0), 0);
    RETURN
    CASE
        WHEN v_score >= 1.20 THEN
            'critical'
        WHEN v_score >= 0.80 THEN
            'watch'
        ELSE
            'ok'
    END;
END$$

CREATE PROCEDURE sp_assert_eq_bigint (
    IN p_expected BIGINT,
    IN p_actual BIGINT,
    IN p_message VARCHAR (255)
)
BEGIN
    IF NOT (p_expected <=> p_actual) THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = CONCAT ('ASSERT: ', p_message);
    END IF;
END$$"#;

    let cursor = sql.find("v_score >= 0.80").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    )
    .expect("expected function bounds inside mixed MySQL custom-delimiter script");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE FUNCTION fn_efficiency_band"),
        "cursor inside the function should resolve only the function body, got:\n{statement}"
    );
    assert!(
        statement.contains("DECLARE v_score DECIMAL (12, 4);")
            && statement.contains("WHEN v_score >= 0.80 THEN"),
        "function body should stay intact, got:\n{statement}"
    );
    assert!(
        !statement.contains("CREATE TRIGGER bi_task_log")
            && !statement.contains("CREATE PROCEDURE sp_assert_eq_bigint"),
        "neighboring custom-delimiter routines must not leak into the current function bounds, got:\n{statement}"
    );
    assert!(
        statement.trim_end().ends_with("END"),
        "statement bounds should stop at the stripped custom delimiter, got:\n{statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_session_delimiter_isolates_second_trigger_without_directive(
) {
    let sql = r#"CREATE TRIGGER bi_task_log
BEFORE INSERT ON task_log
FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL OR NEW.hours <= 0 OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
END$$

CREATE TRIGGER ai_task_log
AFTER INSERT ON task_log
FOR EACH ROW
BEGIN
    INSERT INTO audit_events (
        event_type,
        entity_name,
        entity_id,
        detail
    )
    VALUES (
        'TASK_INSERT',
        'task_log',
        NEW.log_id,
        JSON_OBJECT(
            'project_id', NEW.project_id,
            'employee_id', NEW.employee_id,
            'hours', NEW.hours,
            'work_date', DATE_FORMAT(NEW.work_date, '%Y-%m-%d'),
            'note', NEW.note
        )
    );
END$$"#;

    let cursor = sql.find("INSERT INTO audit_events").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type_with_mysql_delimiter(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
        Some("$$"),
    )
    .expect("expected second trigger bounds with session mysql delimiter");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE TRIGGER ai_task_log"),
        "cursor inside second trigger should isolate only that trigger, got:\n{statement}"
    );
    assert!(
        statement.contains("INSERT INTO audit_events")
            && !statement.contains("CREATE TRIGGER bi_task_log"),
        "first trigger must not leak into second trigger bounds, got:\n{statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_custom_delimiter_ignores_comment_decoys() {
    let sql = r#"DELIMITER //
CREATE PROCEDURE p(IN p_id INT)
BEGIN
    DECLARE v_exists INT DEFAULT 0;
    /* delimiter-looking text inside a comment must stay inert:
       fake END//
       fake DELIMITER ;
    */
    SELECT COUNT(*)
      INTO v_exists
      FROM items
     WHERE id = p_id;
END//
DELIMITER ;"#;
    let cursor = sql.find("INTO v_exists").expect("cursor");
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MariaDB),
    )
    .expect("expected procedure bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("CREATE PROCEDURE p"),
        "comment decoys must not detach the routine body from its header, got:\n{statement}"
    );
    assert!(
        statement.contains("DECLARE v_exists") && statement.trim_end().ends_with("END"),
        "the full routine should remain in the selected statement bounds, got:\n{statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_use_command_isolated_from_following_statement() {
    let sql = "USE reporting\nSELECT 1;\n";
    let cursor = sql.find("SELECT 1").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected SELECT bounds after MySQL USE command");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "SELECT 1",
        "MySQL USE command must not be merged into the following executable statement: {statement:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_source_alias_isolated_from_following_statement() {
    let sql = "\\. /tmp/mysql-bootstrap.sql\nSELECT 1;\n";
    let cursor = sql.find("SELECT 1").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    )
    .expect("expected SELECT bounds after MySQL source alias");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "SELECT 1",
        "MySQL source alias must not be merged into the following executable statement: {statement:?}"
    );
}

#[test]
fn test_split_script_items_mysql_backslash_g_terminator_splits_statements() {
    let sql = "SELECT 1\\G\nSELECT 2;\n";
    let items = QueryExecutor::split_script_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec!["SELECT 1", "SELECT 2"],
        "MySQL \\G terminator should split statements and be stripped from execution SQL: {stmts:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mysql_backslash_g_terminator_isolated_from_next_statement() {
    let sql = "SELECT 1\\G\nSELECT 2;\n";
    let cursor = sql.find("SELECT 2").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    )
    .expect("expected statement bounds after MySQL \\G terminator");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "SELECT 2",
        "statement after MySQL \\G terminator should not absorb previous statement: {statement:?}"
    );
}

#[test]
fn test_split_script_items_mysql_backslash_g_terminator_auto_detects_mysql_mode() {
    let sql = "SELECT 1\\G\nSELECT 2;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec!["SELECT 1", "SELECT 2"],
        "auto-detected MySQL mode should treat \\G as a statement terminator: {stmts:?}"
    );
}

#[test]
fn test_parse_tool_command_mysql_delimiter_accepts_leading_block_comment() {
    let command = QueryExecutor::parse_tool_command("/* note */ DELIMITER /* gap */ $$ -- keep");

    assert!(matches!(
        command,
        Some(crate::db::ToolCommand::MysqlDelimiter { delimiter }) if delimiter == "$$"
    ));
}

#[test]
fn test_split_script_items_mysql_delimiter_ignores_delimiter_inside_trailing_comment() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE demo_proc()
BEGIN
  SELECT 1; -- $$ must stay inside the procedure body
  SELECT 2;
END$$
DELIMITER ;
SELECT 3;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "delimiter text inside trailing comments must not terminate the routine early: {stmts:?}"
    );
    assert!(
        stmts[0].contains("SELECT 2;") && stmts[0].contains("END"),
        "routine body should stay intact until the real END delimiter: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_script_items_mysql_delimiter_allows_inline_comment_after_terminator() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE demo_proc()
BEGIN
  SELECT 1;
END$$ -- routine end marker
DELIMITER ;
SELECT 3;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "inline comments after the delimiter should still terminate the routine: {stmts:?}"
    );
    assert!(
        stmts[0].contains("routine end marker"),
        "comment after the delimiter should be preserved with the statement: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_script_items_mysql_delimiter_ignores_hash_comment_torture_text() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE demo_proc()
BEGIN
# line comment torture: ; ; ; DELIMITER $$ should not matter here.....
SELECT 1;
END$$
DELIMITER ;
SELECT 2;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "hash-comment delimiter script should yield two statements: {stmts:?}"
    );
    assert!(
        stmts[0].contains("# line comment torture: ; ; ; DELIMITER $$ should not matter here....."),
        "hash comment text should stay inside the routine body: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT 1;"),
        "routine body should preserve statements after the hash comment: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 2");
}

#[test]
fn test_split_script_items_mysql_hash_comment_ignores_semicolon_inside_comment() {
    let sql = r#"SELECT 1 # keep ; inside comment
+ 2;
SELECT 3;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "semicolon inside MySQL # comment must not terminate the statement early: {stmts:?}"
    );
    assert!(
        stmts[0].contains("+ 2"),
        "continued arithmetic line should stay in the first statement: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_script_items_mysql_hash_comment_after_identifier_auto_detects_mysql_mode() {
    let sql = r#"SELECT col# keep ; inside comment
+ 2;
SELECT 3;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "identifier-adjacent MySQL # comment should still auto-enable MySQL splitting: {stmts:?}"
    );
    assert!(
        stmts[0].contains("+ 2"),
        "continued arithmetic line should stay in the first statement when # follows an identifier: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_script_items_mysql_hash_after_backslash_escaped_quote_stays_string_content() {
    let sql = "SELECT 'abc\\'#still string';\nSELECT 2;\n";
    let items = QueryExecutor::split_script_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec!["SELECT 'abc\\'#still string'", "SELECT 2"],
        "MySQL split must keep # after an escaped quote inside the string literal: {stmts:?}"
    );
}

#[test]
fn test_split_format_items_mysql_hash_comment_ignores_semicolon_inside_comment() {
    let sql = r#"SELECT 1 # keep ; inside comment
+ 2;
SELECT 3;
"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(statement) => Some(statement.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "format split should ignore semicolons inside MySQL # comments: {stmts:?}"
    );
    assert!(
        stmts[0].contains("+ 2"),
        "formatter split should keep the continued line with the first statement: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_format_items_mysql_hash_comment_after_identifier_auto_detects_mysql_mode() {
    let sql = r#"SELECT col# keep ; inside comment
+ 2;
SELECT 3;
"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(statement) => Some(statement.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "format split should keep identifier-adjacent MySQL # comments inside the statement: {stmts:?}"
    );
    assert!(
        stmts[0].contains("+ 2"),
        "format split should preserve the continued line after an identifier-adjacent # comment: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 3");
}

#[test]
fn test_split_format_items_mysql_quoted_identifier_shields_comment_markers_after_semicolon() {
    let sql = "CREATE TABLE demo (\n\
  `payload;--/*#*/` VARCHAR(120) NOT NULL,\n\
  `doubled``;--name` INT NOT NULL,\n\
  \"doubled\"\";--name\" INT NOT NULL\n\
);";

    let items = QueryExecutor::split_format_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(statement) => Some(statement.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        statements.len(),
        1,
        "quoted identifier content must not become a terminator plus trailing comment: {items:?}"
    );
    for identifier in [
        "`payload;--/*#*/`",
        "`doubled``;--name`",
        "\"doubled\"\";--name\"",
    ] {
        assert!(
            statements[0].contains(identifier),
            "format split lost quoted identifier {identifier}: {}",
            statements[0]
        );
    }
}

#[test]
fn test_split_script_items_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
    let sql = "SELECT 5--2;\nSELECT 9;\n";
    let items = QueryExecutor::split_script_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec!["SELECT 5--2", "SELECT 9"],
        "MySQL db-type split must keep `--2` as arithmetic and preserve the next statement: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_for_oracle_keeps_vector_operators_inside_expressions() {
    let sql = r#"CREATE PROPERTY GRAPH vector_graph
EDGE TABLES (vector_edges
    SOURCE KEY (source_id) REFERENCES vector_items (target_id)
    DESTINATION KEY (target_id) REFERENCES vector_items (target_id)
);

SELECT target_id,
       FROM_VECTOR(embedding + VECTOR('[1, 1, 1]', 3, FLOAT32)) AS vector_sum,
       FROM_VECTOR(embedding * VECTOR('[2, 3, 4]', 3, FLOAT32)) AS vector_product,
       ROUND(embedding <-> VECTOR('[1, 1, 1]', 3, FLOAT32), 6) AS l2_distance,
       ROUND(embedding <=> VECTOR('[1, 1, 1]', 3, FLOAT32), 6) AS cosine_distance,
       ROUND(embedding <#> VECTOR('[1, 1, 1]', 3, FLOAT32), 6) AS neg_dot_product
FROM vector_items
ORDER BY target_id;

SELECT FROM_VECTOR(AVG(embedding)) AS centroid
FROM vector_items;"#;
    let items = QueryExecutor::split_script_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::Oracle),
    );
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        3,
        "Oracle SOURCE KEY must not enable MySQL parsing, and vector operators must not absorb the following statement: {statements:?}"
    );
    assert!(
        statements[1].contains("<->")
            && statements[1].contains("<=>")
            && statements[1].contains("<#>"),
        "all Oracle vector operators should remain in the first expression: {}",
        statements[1]
    );
    assert_eq!(
        statements[2],
        "SELECT FROM_VECTOR(AVG(embedding)) AS centroid\nFROM vector_items"
    );
}

#[test]
fn test_statement_bounds_at_cursor_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
    let sql = "SELECT 5--2;\nSELECT 9;\n";
    let cursor = sql.find("5--2").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    )
    .expect("expected MySQL statement bounds for double-dash arithmetic");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "SELECT 5--2",
        "MySQL db-type statement bounds must keep `--2` as arithmetic instead of treating it as a comment: {statement:?}"
    );
}

#[test]
fn test_mysql_family_line_leading_select_into_variables_stay_in_the_open_statement() {
    let sql = "WITH collapsed AS (SELECT 1 AS n)\n\
               SELECT COUNT(*), SUM(n)\n\
               INTO\n\
                 @row_count,\n\
                 @total\n\
               FROM collapsed;\n\
               SELECT 2;\n";

    for db_type in [
        crate::db::connection::DatabaseType::MySQL,
        crate::db::connection::DatabaseType::MariaDB,
    ] {
        let items = QueryExecutor::split_script_items_for_db_type(sql, Some(db_type));
        let statements = get_statements(&items);
        assert_eq!(
            statements.len(),
            2,
            "{db_type:?} split line-leading user variables as SQL*Plus commands: {items:?}"
        );
        assert!(
            statements[0].starts_with("WITH collapsed")
                && statements[0].contains("@row_count")
                && statements[0].contains("@total")
                && statements[0].contains("FROM collapsed"),
            "{db_type:?} lost part of SELECT INTO: {}",
            statements[0]
        );

        let cursor = sql.find("collapsed;").expect("collapsed source");
        let bounds =
            QueryExecutor::statement_bounds_at_cursor_for_db_type(sql, cursor, Some(db_type))
                .expect("SELECT INTO statement bounds");
        let statement = &sql[bounds.0..bounds.1];
        assert!(
            statement.trim_start().starts_with("WITH collapsed")
                && statement.contains("@row_count")
                && statement.contains("@total"),
            "{db_type:?} statement bounds started after line-leading user variables: {statement:?}"
        );
    }
}

#[test]
fn test_parse_tool_command_mysql_show_tables_is_mysql_command() {
    let command = QueryExecutor::parse_tool_command("SHOW TABLES")
        .expect("SHOW TABLES should parse as a MySQL tool command");

    assert!(matches!(command, crate::db::ToolCommand::ShowTables));
}

#[test]
fn test_parse_tool_command_mysql_show_errors_without_args_prefers_mysql_variant() {
    let command = QueryExecutor::parse_tool_command("SHOW ERRORS")
        .expect("SHOW ERRORS should parse as a MySQL command when no object is specified");

    assert!(matches!(command, crate::db::ToolCommand::MysqlShowErrors));
}

#[test]
fn test_split_format_items_mysql_show_diagnostics_limit_stays_server_sql() {
    let sql = "SHOW WARNINGS LIMIT 0, 5;\nSHOW ERRORS LIMIT 7;";

    for db_type in [
        crate::db::connection::DatabaseType::MySQL,
        crate::db::connection::DatabaseType::MariaDB,
    ] {
        let items = QueryExecutor::split_format_items_for_db_type(sql, Some(db_type));
        let statements = items
            .iter()
            .filter_map(|item| match item {
                FormatItem::Statement(statement) => Some(statement.as_str()),
                FormatItem::ToolCommand(_) | FormatItem::Verbatim(_) | FormatItem::Slash => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            statements.len(),
            2,
            "{db_type:?} SHOW ... LIMIT forms must remain two server SQL statements: {items:?}"
        );
        assert!(
            statements[0].contains("SHOW WARNINGS LIMIT 0, 5")
                && statements[1].contains("SHOW ERRORS LIMIT 7"),
            "{db_type:?} diagnostics LIMIT clauses must not be parsed as client commands: {items:?}"
        );
        assert!(
            items
                .iter()
                .all(|item| !matches!(item, FormatItem::ToolCommand(_))),
            "{db_type:?} diagnostics LIMIT clauses must keep their operands: {items:?}"
        );
    }
}

#[test]
fn test_parse_tool_command_mysql_show_columns_with_schema_round_trips() {
    let command = QueryExecutor::parse_tool_command("SHOW COLUMNS FROM orders FROM sales")
        .expect("SHOW COLUMNS ... FROM <schema> should parse as a MySQL tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::ShowColumns {
            table,
            schema: Some(schema_name)
        } if table == "orders" && schema_name == "sales"
    ));
}

#[test]
fn test_parse_tool_command_mysql_use_preserves_quoted_identifier_and_ignores_comment() {
    let command = QueryExecutor::parse_tool_command("USE `qt final-boss` -- keep current db")
        .expect("quoted MySQL USE command should parse as a tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::Use { database } if database == "`qt final-boss`"
    ));
}

#[test]
fn test_parse_tool_command_mysql_use_accepts_leading_block_comment_and_tab_separator() {
    let command = QueryExecutor::parse_tool_command("/* keep */ USE\t`qt final-boss` # note")
        .expect("MySQL USE with leading block comment and tab should parse as a tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::Use { database } if database == "`qt final-boss`"
    ));
}

#[test]
fn test_parse_tool_command_mysql_show_create_table_preserves_quoted_identifier_and_comment() {
    let command =
        QueryExecutor::parse_tool_command("SHOW CREATE TABLE `order summary` # keep comment")
            .expect("quoted SHOW CREATE TABLE should parse as a MySQL tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::ShowCreateTable { table } if table == "`order summary`"
    ));
}

#[test]
fn test_parse_tool_command_mysql_show_variables_like_keeps_spaced_literal() {
    let command = QueryExecutor::parse_tool_command("SHOW VARIABLES LIKE 'sql mode %'");

    assert!(matches!(
        command,
        Some(crate::db::ToolCommand::ShowVariables { filter: Some(filter) })
            if filter == "sql mode %"
    ));
}

#[test]
fn test_parse_tool_command_mysql_source_keeps_double_dash_in_path_and_strips_hash_comment() {
    let command = QueryExecutor::parse_tool_command("SOURCE /tmp/mysql--seed.sql # note")
        .expect("MySQL SOURCE should parse as a tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::MysqlSource { path } if path == "/tmp/mysql--seed.sql"
    ));
}

#[test]
fn test_parse_tool_command_mysql_source_alias_backslash_dot_is_supported() {
    let command = QueryExecutor::parse_tool_command("\\. /tmp/mysql-init.sql # note")
        .expect("MySQL \\. alias should parse as a SOURCE tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::MysqlSource { path } if path == "/tmp/mysql-init.sql"
    ));
}

#[test]
fn test_parse_tool_command_mysql_source_quoted_path_with_spaces_round_trips() {
    let command = QueryExecutor::parse_tool_command("SOURCE '/tmp/mysql init.sql' # note")
        .expect("quoted MySQL SOURCE path should parse as a tool command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::MysqlSource { path } if path == "/tmp/mysql init.sql"
    ));
}

#[test]
fn test_parse_tool_command_mysql_show_columns_with_like_stays_raw_statement() {
    let command = QueryExecutor::parse_tool_command("SHOW COLUMNS FROM orders LIKE 'id%'");

    assert!(
        command.is_none(),
        "SHOW COLUMNS ... LIKE must stay as raw SQL so the filter clause is preserved"
    );
}

#[test]
fn test_parse_tool_command_show_errors_with_object_keeps_oracle_variant() {
    let command = QueryExecutor::parse_tool_command("SHOW ERRORS PROCEDURE demo_proc")
        .expect("SHOW ERRORS <type> <name> should remain the Oracle client command");

    assert!(matches!(
        command,
        crate::db::ToolCommand::ShowErrors {
            object_type: Some(_),
            object_name: Some(_)
        }
    ));
}

#[test]
fn test_select_with_case_expressions_separated_by_plus() {
    let sql = "SELECT CASE WHEN a=1 THEN 1 ELSE 0 END + CASE WHEN b=2 THEN 1 ELSE 0 END FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_select_with_case_expressions_separated_by_minus_and_division() {
    let sql = "SELECT CASE WHEN a=1 THEN 1 ELSE 0 END - CASE WHEN b=2 THEN 1 ELSE 0 END / CASE WHEN c=3 THEN 1 ELSE 0 END FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "Should have 1 statement, got: {:?}", stmts);
}

#[test]
fn test_line_block_depths_if_with_case_expressions_separated_by_plus() {
    let sql = r#"if 1=1 then
    begin
        select
            case
                when 1=1 then 1
                else 0
            end + case
                when 1=1 then 1
                else 0
            end
        from dual;
end
end if;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 2, 3, 3, 2, 3, 3, 2, 2, 1, 0];

    assert_eq!(
        depths, expected,
        "IF/BEGIN + arithmetic CASE depth tracking mismatch"
    );
}

#[test]
fn test_split_script_items_end_comment_if_continuation() {
    let sql = r#"BEGIN
  IF 1 = 1 THEN
    NULL;
  END /* keep continuation */ IF;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "END comment IF should remain one statement");
}

#[test]
fn test_split_script_items_repeat_block() {
    let sql = r#"DECLARE
  v_count NUMBER := 0;
BEGIN
  REPEAT
    v_count := v_count + 1;
  UNTIL v_count >= 3
  END REPEAT;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "REPEAT block should be one statement");
}

#[test]
fn test_split_script_items_repeat_block_with_end_repeat_on_next_line() {
    let sql = r#"DECLARE
  v_count NUMBER := 0;
BEGIN
  REPEAT
    v_count := v_count + 1;
  UNTIL v_count >= 3
  END
  REPEAT;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "REPEAT block should be one statement");
    assert!(stmts[0].contains("END") && stmts[0].contains("REPEAT"));
}

#[test]
fn test_split_script_items_pipelined_function() {
    let sql = r#"CREATE OR REPLACE FUNCTION stream_numbers(
  p_limit NUMBER
) RETURN SYS.ODCINUMBERLIST PIPELINED
IS
BEGIN
  NULL;
END;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 1, "PIPELINED function should be one statement");
    assert!(stmts[0].contains("PIPELINED"));
}

#[test]
fn test_line_block_depths_increase_for_repeat_loop() {
    let sql = r#"BEGIN
  REPEAT
    NULL;
  UNTIL i > 1
  END REPEAT;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 2, 1, 0];
    assert_eq!(depths, expected, "REPEAT depth tracking mismatch");
}

#[test]
fn test_line_block_depths_with_split_end_repeat() {
    let sql = r#"BEGIN
  REPEAT
    NULL;
  UNTIL i > 1
  END
  REPEAT;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 2, 1, 1, 0];
    assert_eq!(depths, expected, "REPEAT depth tracking mismatch");
}

#[test]
fn test_line_block_depths_increase_for_while_loop() {
    let sql = r#"BEGIN
  WHILE i < 5 LOOP
    i := i + 1;
  END LOOP;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 0];
    assert_eq!(depths, expected, "WHILE LOOP depth tracking mismatch");
}

#[test]
fn test_line_block_depths_with_split_end_while() {
    let sql = r#"BEGIN
  WHILE i < 5 LOOP
    i := i + 1;
  END
  WHILE;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(depths, expected, "END WHILE depth tracking mismatch");
}

#[test]
fn test_line_block_depths_while_do_loop() {
    let sql = r#"BEGIN
  WHILE i < 5 DO
    i := i + 1;
  END WHILE;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 0];
    assert_eq!(depths, expected, "WHILE DO depth tracking mismatch");
}

#[test]
fn test_line_block_depths_with_split_end_while_do() {
    let sql = r#"BEGIN
  WHILE i < 5 DO
    i := i + 1;
  END
  WHILE;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(depths, expected, "split END WHILE after WHILE DO mismatch");
}

#[test]
fn test_line_block_depths_end_while_does_not_arm_new_do_block() {
    let sql = r#"BEGIN
  WHILE i < 5 DO
    i := i + 1;
  END WHILE;
  DO 1;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "END WHILE must not set pending WHILE-DO state for the next DO"
    );
}

#[test]
fn test_line_block_depths_for_do_loop() {
    let sql = r#"BEGIN
  FOR i IN 1..3 DO
    NULL;
  END FOR;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 0];
    assert_eq!(depths, expected, "FOR DO depth tracking mismatch");
}

#[test]
fn test_line_block_depths_with_split_end_for_do() {
    let sql = r#"BEGIN
  FOR i IN 1..3 DO
    NULL;
  END
  FOR;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(depths, expected, "split END FOR after FOR DO mismatch");
}

#[test]
fn test_line_block_depths_end_for_does_not_arm_new_do_block() {
    let sql = r#"BEGIN
  FOR i IN 1..3 DO
    NULL;
  END FOR;
  DO 1;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "END FOR must not set pending FOR-DO state for the next DO"
    );
}

#[test]
fn test_line_block_depths_preserve_pending_end_across_blank_line() {
    let sql = r#"BEGIN
  WHILE i < 5 LOOP
    i := i + 1;
  END

  WHILE;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "blank line between END and WHILE should keep END pending"
    );
}

#[test]
fn test_line_block_depths_preserve_pending_end_across_comment_line() {
    let sql = r#"BEGIN
  WHILE i < 5 LOOP
    i := i + 1;
  END
  -- keep END pending for next keyword
  WHILE;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "comment line between END and WHILE should keep END pending"
    );
}

#[test]
fn test_line_block_depths_plain_identifier_after_end_is_not_label_continuation() {
    let sql = r#"BEGIN
  IF cond THEN
    NULL;
  END
  value := 1;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "identifier-starting statement after END must not be treated as split END label continuation"
    );
}

#[test]
fn test_line_block_depths_dotted_identifier_after_end_is_not_label_continuation() {
    let sql = r#"BEGIN
  IF cond THEN
    NULL;
  END
  pkg.proc();
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 2, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "dotted call after END must not be treated as split END label continuation"
    );
}

#[test]
fn test_line_block_depths_with_for_update_clause() {
    let sql = r#"SELECT id, status
FROM jobs
WHERE status = 'PENDING'
FOR UPDATE SKIP LOCKED;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    assert_eq!(depths, vec![0, 0, 0, 0]);
}

#[test]
fn test_line_block_depths_for_update_inside_block_does_not_arm_do_block() {
    let sql = r#"BEGIN
  SELECT id
  INTO v_id
  FROM jobs
  WHERE status = 'PENDING'
  FOR UPDATE;
  DO 1;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 1, 1, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "FOR UPDATE inside a block must not leave pending FOR-DO state for a later DO"
    );
}

#[test]
fn test_split_script_items_trigger_for_each_row_does_not_arm_for_do() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_jobs
BEFORE INSERT ON jobs
FOR EACH ROW
BEGIN
  DO 1;
END;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        2,
        "FOR EACH ROW in trigger header must not arm FOR ... DO state: {:?}",
        statements
    );
    assert!(
        statements[0].contains("CREATE OR REPLACE TRIGGER"),
        "first statement should be the trigger body"
    );
    assert!(
        statements[1].contains("SELECT 1 FROM dual"),
        "second statement should remain independently split"
    );
}

#[test]
fn test_line_block_depths_with_with_clause_prefixed_by_hint_comment() {
    let sql = r#"/*+ leading_optimizer_hint */ WITH cte AS (
  SELECT 1 AS id FROM dual
)
SELECT * FROM cte;"#;
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let mut with_idx = None;
    let mut cte_select_idx = None;
    let mut main_select_idx = None;

    for (idx, line) in lines.iter().enumerate() {
        if with_idx.is_none() && line.to_uppercase().starts_with("/*") {
            if line.to_uppercase().contains(" WITH ") {
                with_idx = Some(idx);
            }
        } else if with_idx.is_none()
            && line.to_uppercase().trim_start().starts_with("WITH ")
            && !line.trim_start().starts_with("--")
        {
            with_idx = Some(idx);
        }

        if line.to_uppercase().trim_start().starts_with("SELECT ") && cte_select_idx.is_none() {
            cte_select_idx = Some(idx);
        } else if line
            .to_uppercase()
            .trim_start()
            .starts_with("SELECT * FROM CTE")
            && main_select_idx.is_none()
        {
            main_select_idx = Some(idx);
        }
    }

    let with_idx = with_idx.expect("expected hint-prefixed WITH line");
    let cte_select_idx = cte_select_idx.expect("expected CTE SELECT");
    let main_select_idx = main_select_idx.expect("expected main SELECT after CTE");

    assert!(
        with_idx + 1 < lines.len(),
        "WITH line should have body line"
    );
    assert!(
        depths[with_idx + 1] > depths[with_idx],
        "WITH body should be indented deeper than hint+WITH line"
    );
    assert!(
        depths[cte_select_idx] > depths[with_idx],
        "CTE SELECT should be indented under WITH"
    );
    assert!(
        depths[main_select_idx] <= depths[with_idx],
        "Main SELECT should be dedented to CTE scope"
    );
}

#[test]
fn test_line_block_depths_works_with_sqlplus_comment_between_with_parenthesis_and_select() {
    let sql = r#"WITH cte AS (
REM first line of cte is comment
SELECT 1 AS id
FROM dual
)
SELECT * FROM cte;"#;
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let mut with_idx = None;
    let mut cte_select_idx = None;
    let mut main_select_idx = None;

    for (idx, line) in lines.iter().enumerate() {
        if with_idx.is_none() && line.trim_start().to_uppercase().starts_with("WITH ") {
            with_idx = Some(idx);
        }

        if cte_select_idx.is_none() && line.trim_start().to_uppercase().starts_with("SELECT 1 AS") {
            cte_select_idx = Some(idx);
        } else if main_select_idx.is_none()
            && line
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        {
            main_select_idx = Some(idx);
        }
    }

    let with_idx = with_idx.expect("expected WITH clause line");
    let cte_select_idx = cte_select_idx.expect("expected CTE SELECT line");
    let main_select_idx = main_select_idx.expect("expected main SELECT line");

    assert!(
        depths[cte_select_idx] > depths[with_idx],
        "CTE SELECT should be indented deeper than WITH line"
    );
    assert!(
        depths[main_select_idx] <= depths[with_idx],
        "Main SELECT should be dedented back to query scope"
    );
    assert!(
        depths[cte_select_idx] > depths[main_select_idx],
        "CTE SELECT should be deeper than main SELECT"
    );
}

#[test]
fn test_line_block_depths_q_quote_invalid_apostrophe_delimiter_does_not_swallow_next_line() {
    let sql = "SELECT q'' AS val FROM dual;
SELECT 2 FROM dual;";
    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(depths.len(), 2, "unexpected depths: {depths:?}");
    assert_eq!(depths[0], 0, "first SELECT should stay at top level");
    assert_eq!(depths[1], 0, "second SELECT should stay at top level");
}

#[test]
fn test_split_script_items_q_quote_invalid_apostrophe_delimiter_splits_normally() {
    let sql = "SELECT q'' AS val FROM dual;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    let stmts: Vec<String> = items
        .iter()
        .filter_map(|item| {
            if let ScriptItem::Statement(s) = item {
                Some(s.trim().to_string())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(stmts.len(), 2, "unexpected statements: {stmts:?}");
    assert_eq!(stmts[0], "SELECT q'' AS val FROM dual".to_string());
    assert_eq!(stmts[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn test_line_block_depths_increase_for_loop_subquery_with_and_package() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_demo AS
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

    let depths = QueryExecutor::line_block_depths(sql);

    // PACKAGE BODY +1
    assert!(depths[1] >= 1, "Package body should increase depth");
    // PROCEDURE/FUNCTION BEGIN +1
    assert!(
        depths[3] > depths[2],
        "Procedure BEGIN should increase depth"
    );
    // Subquery (SELECT ...) +1
    assert!(
        depths[6] > depths[5],
        "Nested subquery should increase depth"
    );
    // LOOP ... END LOOP +1
    assert!(
        depths[9] > depths[8],
        "LOOP body should be deeper than LOOP line"
    );
    // WITH CTE block +1
    assert!(
        depths[15] > depths[14],
        "CTE body should be indented under WITH"
    );
}

#[test]
fn test_line_block_depths_with_with_clause_followed_by_update() {
    let sql = r#"WITH cte AS (
  SELECT 1 AS id FROM dual
)
UPDATE demo_table
SET id = 1
WHERE EXISTS (
  SELECT 1 FROM cte
);"#;

    let depths = QueryExecutor::line_block_depths(sql);

    let lines: Vec<&str> = sql.lines().collect();
    let mut with_idx = None;
    let mut update_idx = None;
    let mut where_idx = None;
    let mut exists_select_idx = None;

    for (idx, line) in lines.iter().enumerate() {
        let upper = line.trim().to_uppercase();
        if with_idx.is_none() && upper.starts_with("WITH ") {
            with_idx = Some(idx);
        } else if upper.starts_with("UPDATE ") {
            update_idx = Some(idx);
        } else if upper.starts_with("WHERE ") {
            where_idx = Some(idx);
        } else if idx > 0
            && lines[idx - 1]
                .trim()
                .to_uppercase()
                .starts_with("WHERE EXISTS (")
            && upper.starts_with("SELECT ")
        {
            exists_select_idx = Some(idx);
        }
    }

    let with_idx = with_idx.expect("expected WITH clause line");
    let update_idx = update_idx.expect("expected UPDATE line");
    let where_idx = where_idx.expect("expected WHERE line");
    let exists_select_idx = exists_select_idx.expect("expected nested EXISTS SELECT line");

    assert!(
        with_idx + 1 < depths.len(),
        "CTE should have at least two lines"
    );
    assert!(
        depths[with_idx + 1] > depths[with_idx],
        "CTE body SELECT should be deeper than WITH header"
    );
    assert!(
        depths[update_idx] <= depths[with_idx],
        "Main UPDATE should dedent out of WITH body"
    );
    assert!(
        depths[exists_select_idx] > depths[where_idx],
        "EXISTS subquery SELECT should be nested"
    );
}

#[test]
fn test_line_block_depths_ignores_subquery_pattern_inside_string_literal() {
    let sql = r#"BEGIN
  v_sql := '(SELECT';
  NULL;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "String literal '(SELECT' should not affect subquery depth tracking"
    );
}

#[test]
fn test_line_block_depths_ignores_subquery_pattern_inside_dollar_quote() {
    let sql = r#"BEGIN
  v_sql := $tag$(SELECT BEGIN END)$tag$;
  NULL;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "Dollar-quoted '(SELECT BEGIN END)' should not affect line depth tracking"
    );
}

#[test]
fn test_line_block_depths_ignores_subquery_pattern_inside_block_comment() {
    let sql = r#"BEGIN
  /* (SELECT */
  NULL;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "Block comment '(SELECT' should not affect subquery depth tracking"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_inline_block_comment() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (/* inline note */ SELECT
  1
FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let select_one_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with('1'))
        .expect("expected SELECT body line");

    assert!(
        depths[select_one_idx] > depths[where_idx],
        "Inline block comment before SELECT should still preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_leading_block_comment_with_sql_same_line() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  /* comment */ SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("/* comment */ SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "Block comment prefix before SELECT should still preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_leading_hint_comment_with_sql_same_line() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  /*+ qb_name(inner_q) */ SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .starts_with("/*+ qb_name(inner_q) */ SELECT 1")
        })
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "Hint comment prefix before SELECT should still preserve subquery depth"
    );
}

// ── parse_ddl_object_type tests ──

#[test]
fn test_parse_ddl_object_type_create_table() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE TABLE MY_TABLE (ID NUMBER)"),
        "Table"
    );
}

#[test]
fn test_parse_ddl_object_type_create_global_temp_table() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE GLOBAL TEMPORARY TABLE MY_TABLE (ID NUMBER)"),
        "Table"
    );
}

#[test]
fn test_parse_ddl_object_type_create_view() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE VIEW MY_VIEW AS SELECT 1 FROM DUAL"),
        "View"
    );
}

#[test]
fn test_parse_ddl_object_type_create_materialized_view() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE MATERIALIZED VIEW MY_MV AS SELECT 1 FROM DUAL"
        ),
        "View"
    );
}

#[test]
fn test_parse_ddl_object_type_create_index() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE INDEX MY_IDX ON MY_TABLE(ID)"),
        "Index"
    );
}

#[test]
fn test_parse_ddl_object_type_create_unique_index() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE UNIQUE INDEX MY_IDX ON MY_TABLE(ID)"),
        "Index"
    );
}

#[test]
fn test_parse_ddl_object_type_create_procedure() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE PROCEDURE MY_PROC AS BEGIN NULL; END;"),
        "Procedure"
    );
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_procedure() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE PROCEDURE MY_PROC AS BEGIN NULL; END;"
        ),
        "Procedure"
    );
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_force_procedure() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE FORCE PROCEDURE MY_PROC AS BEGIN NULL; END;"
        ),
        "Procedure"
    );
}

#[test]
fn test_parse_ddl_object_type_create_no_force_procedure() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE NO FORCE PROCEDURE MY_PROC AS BEGIN NULL; END;"
        ),
        "Procedure"
    );
}

#[test]
fn test_parse_ddl_object_type_create_function() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE FUNCTION MY_FUNC RETURN NUMBER IS BEGIN RETURN 1; END;"
        ),
        "Function"
    );
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_function() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE FUNCTION MY_FUNC RETURN NUMBER IS BEGIN RETURN 1; END;"
        ),
        "Function"
    );
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_editionable_force_function() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE EDITIONABLE FORCE FUNCTION MY_FUNC RETURN NUMBER IS BEGIN RETURN 1; END;"
        ),
        "Function"
    );
}

#[test]
fn test_parse_ddl_object_type_create_package() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE PACKAGE MY_PKG AS PROCEDURE PROC1; END MY_PKG;"
        ),
        "Package"
    );
}

#[test]
fn test_parse_ddl_object_type_create_package_body() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE PACKAGE BODY MY_PKG AS PROCEDURE PROC1 IS BEGIN NULL; END; END MY_PKG;"
        ),
        "Package Body"
    );
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_type_body() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE TYPE BODY MY_TYPE AS MEMBER FUNCTION GET_ID RETURN NUMBER IS BEGIN RETURN ID; END;"
        ),
        "Type Body"
    );
}

#[test]
fn test_parse_ddl_object_type_create_trigger() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE TRIGGER MY_TRIG BEFORE INSERT ON MY_TABLE BEGIN NULL; END;"
        ),
        "Trigger"
    );
}

#[test]
fn test_parse_ddl_object_type_create_noforce_trigger() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE NOFORCE TRIGGER MY_TRIG BEFORE INSERT ON MY_TABLE BEGIN NULL; END;"
        ),
        "Trigger"
    );
}

#[test]
fn test_parse_ddl_object_type_create_forward_crossedition_trigger() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE FORWARD CROSSEDITION TRIGGER MY_TRIG BEFORE INSERT ON MY_TABLE BEGIN NULL; END;"
        ),
        "Trigger"
    );
}

#[test]
fn test_parse_ddl_object_type_create_reverse_crossedition_trigger() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE REVERSE CROSSEDITION TRIGGER MY_TRIG BEFORE INSERT ON MY_TABLE BEGIN NULL; END;"
        ),
        "Trigger"
    );
}

#[test]
fn test_parse_ddl_object_type_create_sharing_data_package() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE SHARING=DATA PACKAGE MY_PKG AS PROCEDURE P; END MY_PKG;"
        ),
        "Package"
    );
}

#[test]
fn test_parse_ddl_object_type_create_sharing_equals_metadata_package() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE SHARING = METADATA PACKAGE MY_PKG AS PROCEDURE P; END MY_PKG;"
        ),
        "Package"
    );
}

#[test]
fn test_parse_ddl_object_type_create_sequence() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE SEQUENCE MY_SEQ START WITH 1"),
        "Sequence"
    );
}

#[test]
fn test_parse_ddl_object_type_create_synonym() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE SYNONYM MY_SYN FOR OTHER_TABLE"),
        "Synonym"
    );
}

#[test]
fn test_parse_ddl_object_type_create_public_synonym() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE PUBLIC SYNONYM MY_SYN FOR OTHER_TABLE"),
        "Synonym"
    );
}

#[test]
fn test_parse_ddl_object_type_create_type() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE TYPE MY_TYPE AS OBJECT (ID NUMBER)"),
        "Type"
    );
}

#[test]
fn test_parse_ddl_object_type_create_type_body() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE TYPE BODY MY_TYPE AS MEMBER FUNCTION GET_ID RETURN NUMBER IS BEGIN RETURN ID; END; END;"),
        "Type Body"
    );
}

#[test]
fn test_parse_ddl_object_type_create_database_link() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS USING 'TNS'"
        ),
        "Database Link"
    );
}

#[test]
fn test_parse_ddl_object_type_create_public_database_link() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE PUBLIC DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS USING 'TNS'"
        ),
        "Database Link"
    );
}

#[test]
fn test_parse_ddl_object_type_create_shared_database_link() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE SHARED DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS USING 'TNS'"
        ),
        "Database Link"
    );
}

#[test]
fn test_parse_ddl_object_type_create_shared_public_database_link() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE SHARED PUBLIC DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS USING 'TNS'"
        ),
        "Database Link"
    );
}

#[test]
fn test_parse_ddl_object_type_alter_database_link_variants() {
    for sql in [
        "ALTER DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS",
        "ALTER PUBLIC DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS",
        "ALTER SHARED DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS",
        "ALTER SHARED PUBLIC DATABASE LINK MY_LINK CONNECT TO USER IDENTIFIED BY PASS",
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            "Database Link",
            "DDL type parser should classify `{sql}` as a database link"
        );
    }
}

#[test]
fn test_parse_ddl_object_type_admin_storage_objects() {
    for (sql, expected) in [
        ("CREATE DIRECTORY DATA_PUMP_DIR AS '/tmp'", "Directory"),
        (
            "CREATE OR REPLACE DIRECTORY DATA_PUMP_DIR AS '/tmp'",
            "Directory",
        ),
        ("DROP DIRECTORY DATA_PUMP_DIR", "Directory"),
        (
            "CREATE TABLESPACE APP_TS DATAFILE 'app01.dbf' SIZE 100M",
            "Tablespace",
        ),
        ("ALTER TABLESPACE APP_TS READ ONLY", "Tablespace"),
        ("DROP TABLESPACE APP_TS", "Tablespace"),
        ("CREATE ROLE APP_READONLY", "Role"),
        ("ALTER ROLE APP_READONLY NOT IDENTIFIED", "Role"),
        ("DROP ROLE APP_READONLY", "Role"),
        (
            "CREATE PROFILE APP_PROFILE LIMIT SESSIONS_PER_USER 4",
            "Profile",
        ),
        (
            "ALTER PROFILE APP_PROFILE LIMIT SESSIONS_PER_USER 8",
            "Profile",
        ),
        ("DROP PROFILE APP_PROFILE", "Profile"),
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            expected,
            "DDL type parser should classify `{sql}` as {expected}"
        );
    }
}

#[test]
fn test_parse_ddl_object_type_rollback_segment_variants() {
    for sql in [
        "CREATE ROLLBACK SEGMENT RBS_ONE TABLESPACE RBS_TS",
        "CREATE PUBLIC ROLLBACK SEGMENT RBS_ONE TABLESPACE RBS_TS",
        "ALTER ROLLBACK SEGMENT RBS_ONE ONLINE",
        "DROP ROLLBACK SEGMENT RBS_ONE",
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            "Rollback Segment",
            "DDL type parser should classify `{sql}` as a rollback segment"
        );
    }
}

#[test]
fn test_parse_ddl_object_type_java_variants() {
    for (sql, expected) in [
        (
            r#"CREATE JAVA SOURCE NAMED "Welcome" AS public class Welcome {}"#,
            "Java Source",
        ),
        (
            r#"CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED "Welcome" AS public class Welcome {}"#,
            "Java Source",
        ),
        (
            r#"CREATE OR REPLACE AND RESOLVE JAVA CLASS USING BFILE (java_dir, 'Agent.class')"#,
            "Java Class",
        ),
        (
            r#"CREATE JAVA RESOURCE NAMED "appText" USING BFILE (java_dir, 'textBundle.dat')"#,
            "Java Resource",
        ),
        (r#"ALTER JAVA SOURCE "Welcome" COMPILE"#, "Java Source"),
        (r#"ALTER JAVA CLASS "Agent" RESOLVE"#, "Java Class"),
        (r#"DROP JAVA SOURCE "Welcome""#, "Java Source"),
        (r#"DROP JAVA CLASS "Agent""#, "Java Class"),
        (r#"DROP JAVA RESOURCE "appText""#, "Java Resource"),
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            expected,
            "DDL type parser should classify `{sql}` as {expected}"
        );
    }
}

#[test]
fn test_parse_ddl_object_type_create_or_replace_editionable_function() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type(
            "CREATE OR REPLACE EDITIONABLE FUNCTION MY_FUNC RETURN NUMBER IS BEGIN RETURN 1; END;"
        ),
        "Function"
    );
}

#[test]
fn test_parse_ddl_object_type_alter_table() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("ALTER TABLE MY_TABLE ADD (COL1 NUMBER)"),
        "Table"
    );
}

#[test]
fn test_parse_ddl_object_type_alter_session() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("ALTER SESSION SET CURRENT_SCHEMA = APP_USER"),
        "Session"
    );
}

#[test]
fn test_parse_ddl_object_type_alter_system() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("ALTER SYSTEM SET OPEN_CURSORS = 1000"),
        "System"
    );
}

#[test]
fn test_parse_ddl_object_type_create_materialized_view_log() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("CREATE MATERIALIZED VIEW LOG ON SALES"),
        "Materialized View Log"
    );
}

#[test]
fn test_parse_ddl_object_type_alter_drop_materialized_view_log() {
    for sql in [
        "ALTER MATERIALIZED VIEW LOG ON SALES ADD ROWID",
        "DROP MATERIALIZED VIEW LOG ON SALES",
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            "Materialized View Log",
            "DDL type parser should classify `{sql}` as a materialized view log"
        );
    }
}

#[test]
fn test_ddl_message_alter_session_current_schema() {
    assert_eq!(
        QueryExecutor::ddl_message("ALTER SESSION SET CURRENT_SCHEMA = APP_USER"),
        "Current schema changed"
    );
}

#[test]
fn test_ddl_message_alter_session_nls_parameter() {
    assert_eq!(
        QueryExecutor::ddl_message("ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'"),
        "Session NLS setting updated"
    );
}

#[test]
fn test_parse_ddl_object_type_drop_procedure() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("DROP PROCEDURE MY_PROC"),
        "Procedure"
    );
}

#[test]
fn test_parse_ddl_object_type_drop_public_synonym() {
    assert_eq!(
        QueryExecutor::parse_ddl_object_type("DROP PUBLIC SYNONYM MY_SYN"),
        "Synonym"
    );
}

#[test]
fn test_parse_ddl_object_type_drop_database_link_variants() {
    for sql in [
        "DROP DATABASE LINK MY_LINK",
        "DROP PUBLIC DATABASE LINK MY_LINK",
    ] {
        assert_eq!(
            QueryExecutor::parse_ddl_object_type(sql),
            "Database Link",
            "DDL type parser should classify `{sql}` as a database link"
        );
    }
}

/// Regression: CREATE FUNCTION with PROCEDURE keyword in body should return "Function"
#[test]
fn test_parse_ddl_object_type_function_with_procedure_in_body() {
    let sql = "CREATE OR REPLACE FUNCTION MY_FUNC RETURN NUMBER IS BEGIN EXECUTE IMMEDIATE 'CALL MY_PROCEDURE ()'; RETURN 1; END;";
    assert_eq!(QueryExecutor::parse_ddl_object_type(sql), "Function");
}

/// Regression: CREATE PACKAGE with FUNCTION/PROCEDURE in body should return "Package"
#[test]
fn test_parse_ddl_object_type_package_with_mixed_body() {
    let sql = "CREATE OR REPLACE PACKAGE MY_PKG AS PROCEDURE PROC1; FUNCTION FUNC1 RETURN NUMBER; END MY_PKG;";
    assert_eq!(QueryExecutor::parse_ddl_object_type(sql), "Package");
}

/// Regression: CREATE TRIGGER with TABLE in body should return "Trigger"
#[test]
fn test_parse_ddl_object_type_trigger_with_table_in_body() {
    let sql = "CREATE OR REPLACE TRIGGER MY_TRIG BEFORE INSERT ON MY_TABLE FOR EACH ROW BEGIN INSERT INTO LOG_TABLE VALUES (SYSDATE); END;";
    assert_eq!(QueryExecutor::parse_ddl_object_type(sql), "Trigger");
}

#[test]
fn test_parse_whenever_oserror_continue() {
    let sql = "WHENEVER OSERROR CONTINUE\nSELECT 1 FROM DUAL;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(
            items.first(),
            Some(ScriptItem::ToolCommand(ToolCommand::WheneverOsError {
                exit: false
            }))
        ),
        "Expected WHENEVER OSERROR CONTINUE tool command, got: {:?}",
        items.first()
    );
}

#[test]
fn test_parse_whenever_oserror_exit() {
    let sql = "WHENEVER OSERROR EXIT\nSELECT 1 FROM DUAL;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(
            items.first(),
            Some(ScriptItem::ToolCommand(ToolCommand::WheneverOsError {
                exit: true
            }))
        ),
        "Expected WHENEVER OSERROR EXIT tool command, got: {:?}",
        items.first()
    );
}

#[test]
fn test_parse_whenever_sqlerror_exit_sql_sqlcode() {
    let sql = "WHENEVER SQLERROR EXIT SQL.SQLCODE\nSELECT 1 FROM DUAL;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(
            items.first(),
            Some(ScriptItem::ToolCommand(ToolCommand::WheneverSqlError {
                exit: true,
                action: Some(action)
            })) if action.eq_ignore_ascii_case("SQL.SQLCODE")
        ),
        "Expected WHENEVER SQLERROR EXIT SQL.SQLCODE tool command, got: {:?}",
        items.first()
    );
}

#[test]
fn test_summarize_batch_results_marks_failure_when_dml_batch_has_errors() {
    let result = QueryExecutor::summarize_batch_results(
        "UPDATE t SET c = 1; BAD SQL;",
        2,
        std::time::Duration::from_millis(12),
        None,
        1,
        1,
        vec!["Statement 2: ORA-00900: invalid SQL statement".to_string()],
    );

    assert!(
        !result.success,
        "batch summary should fail when any statement fails"
    );
    assert!(!result.is_select);
    assert!(result.message.contains("Executed 1 of 2 statements"));
    assert!(result.message.contains("Errors:"));
}

#[test]
fn test_summarize_batch_results_marks_failure_when_select_batch_has_errors() {
    let select_result = QueryResult::new_select(
        "SELECT * FROM dual",
        vec![ColumnInfo {
            name: "DUMMY".to_string(),
            data_type: "VARCHAR2".to_string(),
        }],
        vec![vec!["X".to_string()]],
        std::time::Duration::from_millis(2),
    );

    let result = QueryExecutor::summarize_batch_results(
        "SELECT * FROM dual; BAD SQL;",
        2,
        std::time::Duration::from_millis(20),
        Some(select_result),
        0,
        1,
        vec!["Statement 2: ORA-00900: invalid SQL statement".to_string()],
    );

    assert!(
        !result.success,
        "select batch should fail when any statement fails"
    );
    assert!(result.is_select);
    assert!(result.message.contains("Errors:"));
    assert!(result.message.contains("Executed 1 of 2 statements"));
}

// ── q-quote after identifier: depth / split regression ──

#[test]
fn test_split_script_items_identifier_ending_q_followed_by_string() {
    // `seq` ends in 'q' → the q-quote detector must NOT treat `q'text'` as a q-quote.
    let sql = "SELECT seq'text' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "identifier ending in q followed by string should not confuse the splitter, got: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_identifier_ending_nq_followed_by_string() {
    // `unq` ends in 'nq' → nq-quote detector must NOT fire.
    let sql = "SELECT unq'val' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(
        stmts.len(),
        2,
        "identifier ending in nq followed by string should split correctly, got: {stmts:?}"
    );
}

#[test]
fn test_line_block_depths_identifier_ending_q_with_subquery() {
    // Subquery paren after `seq'text'` must still be tracked.
    let sql = "SELECT seq'x', (SELECT 1 FROM dual)\nFROM t;";
    let depths = QueryExecutor::line_block_depths(sql);
    assert_eq!(
        depths,
        vec![0, 0],
        "subquery depth should not be affected by identifier ending in q before string"
    );
}

#[test]
fn test_statement_bounds_at_cursor_identifier_ending_q_string_keeps_second_statement() {
    let sql = "SELECT seq'v' FROM dual;
SELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected second statement bounds after identifier-ending q literal");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "expected second statement, got: {statement}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_identifier_ending_nq_string_keeps_second_statement() {
    let sql = "SELECT unq'v' FROM dual;
SELECT 2 FROM dual;";
    let cursor = sql.rfind("2 FROM dual").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected second statement bounds after identifier-ending nq literal");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("SELECT 2 FROM dual"),
        "expected second statement, got: {statement}"
    );
}

#[test]
fn test_line_block_depths_real_q_quote_still_works() {
    // Standalone q-quote must continue to work.
    let sql = "SELECT q'[hello]', (SELECT 1 FROM dual)\nFROM t;";
    let depths = QueryExecutor::line_block_depths(sql);
    assert_eq!(
        depths,
        vec![0, 0],
        "standalone q-quote should not break depth"
    );
}

#[test]
fn test_split_script_items_q_quote_whitespace_delimiter_falls_back_to_normal_quote() {
    let sql = "SELECT q' hello' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "q' <space>...' must not be treated as q-quote delimiter and should split normally: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("SELECT q' hello' FROM dual"),
        "first statement should preserve regular quote fallback: {}",
        stmts[0]
    );
}

#[test]
fn test_split_script_items_nq_quote_whitespace_delimiter_falls_back_to_normal_quote() {
    let sql = "SELECT nq' hello' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "nq' <space>...' must not be treated as q-quote delimiter and should split normally: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("SELECT nq' hello' FROM dual"),
        "first statement should preserve regular quote fallback: {}",
        stmts[0]
    );
}

#[test]
fn test_line_block_depths_subquery_headed_by_with_clause_indents_correctly() {
    // When `(` is followed by `WITH cte AS (...) SELECT ...` on the next line,
    // the WITH line and the main SELECT inside the paren must both be at depth 1.
    let sql = "SELECT *\nFROM (\n  WITH cte AS (SELECT 1 FROM dual)\n  SELECT * FROM cte\n);";
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let from_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("FROM ("))
        .unwrap();
    let with_idx = lines
        .iter()
        .position(|l| l.trim_start().to_uppercase().starts_with("WITH "))
        .unwrap();
    let inner_select_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .unwrap();

    assert!(
        depths[with_idx] > depths[from_idx],
        "WITH inside paren should be deeper than outer FROM line (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[inner_select_idx], depths[with_idx],
        "main SELECT after CTE should stay at same depth as WITH (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_subquery_with_clause_multiline_cte_body() {
    // Same as above but with CTE body on its own lines to exercise pending_subquery_paren.
    let sql = "SELECT *\nFROM (\n  WITH cte AS (\n    SELECT 1 AS n FROM dual\n  )\n  SELECT * FROM cte\n);";
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let from_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("FROM ("))
        .unwrap();
    let with_idx = lines
        .iter()
        .position(|l| l.trim_start().to_uppercase().starts_with("WITH "))
        .unwrap();
    let inner_select_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .unwrap();

    assert!(
        depths[with_idx] > depths[from_idx],
        "WITH inside paren must be deeper than outer FROM (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[inner_select_idx], depths[with_idx],
        "main SELECT after multi-line CTE body must match WITH depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_subquery_with_clause_on_same_line_as_open_paren() {
    let sql = "SELECT *\nFROM (WITH cte AS (SELECT 1 AS n FROM dual)\n      SELECT * FROM cte\n);";
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let from_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("FROM (WITH "))
        .unwrap();
    let inner_select_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .unwrap();

    assert!(
        depths[inner_select_idx] > depths[from_idx],
        "main SELECT after same-line (WITH should still stay in nested subquery depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_subquery_with_clause_after_block_comment_same_line() {
    let sql = "SELECT *\nFROM (/* c */ WITH cte AS (SELECT 1 AS n FROM dual)\n      SELECT * FROM cte\n);";
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let from_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("FROM (/* c */ WITH "))
        .unwrap();
    let inner_select_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .unwrap();

    assert!(
        depths[inner_select_idx] > depths[from_idx],
        "block comment between ( and WITH should preserve nested subquery depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_standalone_with_main_select_not_affected_by_fix() {
    // Regression guard: a top-level (non-nested) WITH…SELECT must still give
    // depth 0 for the main SELECT, exactly as before the fix.
    let sql = "WITH cte AS (\n  SELECT 1 AS n FROM dual\n)\nSELECT * FROM cte;";
    let lines: Vec<&str> = sql.lines().collect();
    let depths = QueryExecutor::line_block_depths(sql);

    let with_idx = lines
        .iter()
        .position(|l| l.trim_start().to_uppercase().starts_with("WITH "))
        .unwrap();
    let main_select_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .unwrap();

    assert!(
        depths[with_idx + 1] > depths[with_idx],
        "CTE body must be deeper than WITH line (depths: {:?})",
        depths
    );
    assert!(
        depths[main_select_idx] <= depths[with_idx],
        "main SELECT must dedent back to WITH level (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_line_comment_between_paren_and_select() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  -- comment before nested select
  SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "Line comment between '(' and SELECT should not break subquery depth detection"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_mixed_comments_between_paren_and_select() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  /* first comment */
  -- second comment
  SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "Mixed block/line comments between '(' and SELECT should preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_multiline_block_comment_between_paren_and_select()
{
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  /* comment starts
     and continues without sql keywords
     until this line */
  SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "Multiline block comment between '(' and SELECT should preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_with_subquery_after_multiline_block_comment_between_paren_and_with(
) {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  /* comment starts
     and continues without sql keywords
     until this line */
  WITH cte AS (
    SELECT 1 AS n FROM dual
  )
  SELECT n FROM cte
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let with_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WITH cte AS"))
        .expect("expected nested WITH line");

    assert!(
        depths[with_idx] > depths[where_idx],
        "Multiline block comment between '(' and WITH should preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_rem_comment_between_paren_and_select() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  REM comment before nested select
  SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "REM comment between '(' and SELECT should preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_detects_subquery_after_remark_comment_between_paren_and_select() {
    let sql = r#"SELECT
  col
FROM t
WHERE EXISTS (
  REMARK comment before nested select
  SELECT 1
  FROM dual
);"#;
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let where_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHERE EXISTS"))
        .expect("expected WHERE EXISTS line");
    let nested_select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1"))
        .expect("expected nested SELECT line");

    assert!(
        depths[nested_select_idx] > depths[where_idx],
        "REMARK comment between '(' and SELECT should preserve subquery depth"
    );
}

#[test]
fn test_line_block_depths_with_inside_subquery_in_if_block_keeps_main_select_nested() {
    let sql = r#"BEGIN
  IF 1 = 1 THEN
    SELECT *
    INTO v_dummy
    FROM (
      WITH cte AS (
        SELECT 1 AS n FROM dual
      )
      SELECT * FROM cte
    );
  END IF;
END;
"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let with_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("WITH "))
        .expect("expected WITH line");
    let inner_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM CTE")
        })
        .expect("expected SELECT * FROM cte line");

    assert_eq!(
        depths[inner_select_idx], depths[with_idx],
        "main SELECT after WITH inside subquery should keep nested depth inside IF block (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_multiple_with_subqueries_in_same_plsql_block() {
    let sql = r#"BEGIN
  SELECT *
  INTO v_one
  FROM (
    WITH c1 AS (SELECT 1 AS n FROM dual)
    SELECT * FROM c1
  );

  SELECT *
  INTO v_two
  FROM (
    WITH c2 AS (SELECT 2 AS n FROM dual)
    SELECT * FROM c2
  );
END;
"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let with_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH ")
                .then_some(idx)
        })
        .collect();
    let select_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM C")
                .then_some(idx)
        })
        .collect();

    assert_eq!(with_lines.len(), 2, "expected two WITH lines");
    assert_eq!(select_lines.len(), 2, "expected two SELECT * FROM c* lines");

    for (with_idx, select_idx) in with_lines.iter().zip(select_lines.iter()) {
        assert_eq!(
            depths[*select_idx], depths[*with_idx],
            "each main SELECT after WITH inside subquery should remain nested (depths: {:?})",
            depths
        );
    }
}

#[test]
fn test_line_block_depths_with_values_main_query_dedents_to_with_level() {
    let sql = "WITH cte AS (\n  SELECT 1 AS n\n)\nVALUES ((SELECT n FROM cte));";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let with_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("WITH "))
        .expect("expected WITH line");
    let values_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("VALUES"))
        .expect("expected VALUES line");

    assert_eq!(
        depths[values_idx], depths[with_idx],
        "VALUES main query after WITH should dedent back to WITH depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_nested_with_inside_outer_with_preserves_outer_depth() {
    let sql = "WITH outer_cte AS (\n  WITH inner_cte AS (\n    SELECT 1 AS n FROM dual\n  )\n  SELECT n FROM inner_cte\n),\nouter_two AS (\n  SELECT n FROM outer_cte\n)\nSELECT * FROM outer_two;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let outer_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH OUTER_CTE")
        })
        .expect("expected outer WITH line");
    let inner_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH INNER_CTE")
        })
        .expect("expected inner WITH line");
    let inner_main_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT N FROM INNER_CTE")
        })
        .expect("expected inner main SELECT line");
    let outer_two_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("OUTER_TWO AS (")
        })
        .expect("expected second CTE line");

    assert!(
        depths[inner_with_idx] > depths[outer_with_idx],
        "nested WITH should be deeper than outer WITH (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[inner_main_select_idx], depths[inner_with_idx],
        "inner main SELECT should align with nested WITH depth (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[outer_two_idx],
        depths[outer_with_idx] + 1,
        "after closing inner WITH, next outer CTE should stay in outer WITH CTE body depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_three_level_nested_with_keeps_outer_cte_depth() {
    let sql = "WITH outer_cte AS (\n  WITH mid_cte AS (\n    WITH inner_cte AS (\n      SELECT 1 AS n FROM dual\n    )\n    SELECT n FROM inner_cte\n  )\n  SELECT n FROM mid_cte\n),\nouter_two AS (\n  SELECT n FROM outer_cte\n)\nSELECT * FROM outer_two;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let outer_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH OUTER_CTE")
        })
        .expect("expected outer WITH line");
    let mid_with_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("WITH MID_CTE"))
        .expect("expected middle WITH line");
    let inner_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH INNER_CTE")
        })
        .expect("expected inner WITH line");
    let outer_two_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("OUTER_TWO AS (")
        })
        .expect("expected second outer CTE line");
    let outer_main_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT N FROM MID_CTE")
        })
        .expect("expected outer CTE main SELECT line");

    assert!(
        depths[mid_with_idx] > depths[outer_with_idx],
        "mid WITH should be deeper than outer WITH (depths: {:?})",
        depths
    );
    assert!(
        depths[inner_with_idx] > depths[mid_with_idx],
        "inner WITH should be deeper than mid WITH (depths: {:?})",
        depths
    );
    assert!(
        depths[outer_main_select_idx] > depths[outer_with_idx],
        "outer CTE main SELECT should remain inside outer WITH depth (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[outer_two_idx],
        depths[outer_with_idx] + 1,
        "after nested WITH levels close, next outer CTE should stay in outer WITH CTE body depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_nested_with_in_second_outer_cte_preserves_outer_with_depth() {
    let sql = "WITH outer_one AS (\n  SELECT 1 AS n FROM dual\n),\nouter_two AS (\n  WITH inner_cte AS (\n    SELECT 2 AS n FROM dual\n  )\n  SELECT n FROM inner_cte\n)\nSELECT * FROM outer_two;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let outer_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH OUTER_ONE")
        })
        .expect("expected outer WITH line");
    let outer_two_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("OUTER_TWO AS (")
        })
        .expect("expected OUTER_TWO line");
    let inner_with_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("WITH INNER_CTE")
        })
        .expect("expected inner WITH line");
    let trailing_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT * FROM OUTER_TWO")
        })
        .expect("expected trailing SELECT line");

    assert_eq!(
        depths[outer_two_idx],
        depths[outer_with_idx] + 1,
        "second outer CTE should stay in outer WITH CTE body depth (depths: {:?})",
        depths
    );
    assert!(
        depths[inner_with_idx] > depths[outer_two_idx],
        "nested WITH should be deeper than second outer CTE line (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[trailing_select_idx], depths[outer_with_idx],
        "main SELECT should dedent back to outer WITH header depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_detects_values_subquery_head_after_open_paren() {
    let sql = "SELECT *\nFROM (\n  VALUES (1), (2)\n) AS t(n)\nWHERE n > 1;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM line");
    let values_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("VALUES"))
        .expect("expected VALUES line");

    assert!(
        depths[values_idx] > depths[from_idx],
        "VALUES subquery head should be indented inside FROM parentheses (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_detects_values_subquery_after_comment_between_paren_and_values() {
    let sql = "SELECT *\nFROM (\n  -- comment before nested values\n  VALUES (1), (2)\n) AS t(n)\nWHERE n > 1;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM line");
    let values_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("VALUES"))
        .expect("expected VALUES line");

    assert!(
        depths[values_idx] > depths[from_idx],
        "Comment between '(' and VALUES should preserve nested depth detection"
    );
}

#[test]
fn test_line_block_depths_detects_insert_subquery_head_after_open_paren() {
    let sql = "SELECT *\nFROM (\n  INSERT INTO dst(id) SELECT id FROM src RETURNING id\n) q;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM line");
    let insert_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("INSERT"))
        .expect("expected INSERT line");

    assert!(
        depths[insert_idx] > depths[from_idx],
        "INSERT subquery head should be indented inside FROM parentheses (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_detects_dml_subquery_after_comment_between_paren_and_update() {
    let sql = "SELECT *\nFROM (\n  /* comment before nested update */\n  UPDATE dst SET id = src.id FROM src WHERE dst.id = src.id RETURNING dst.id\n) q;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM line");
    let update_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("UPDATE"))
        .expect("expected UPDATE line");

    assert!(
        depths[update_idx] > depths[from_idx],
        "Comment between '(' and UPDATE should preserve nested depth detection (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_detects_merge_subquery_after_line_comment_between_paren_and_merge() {
    let sql = "SELECT *\nFROM (\n  -- comment before nested merge\n  MERGE INTO dst d USING src s ON (d.id = s.id) WHEN MATCHED THEN UPDATE SET d.id = s.id\n) q;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM line");
    let merge_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("MERGE"))
        .expect("expected MERGE line");

    assert!(
        depths[merge_idx] > depths[from_idx],
        "Line comment between '(' and MERGE should preserve nested depth detection (depths: {:?})",
        depths
    );
}

#[test]
fn test_split_script_items_mysql_if_function_does_not_open_block_depth() {
    let sql = "SELECT IF(score > 90, 'A', 'B') AS grade FROM exam_scores;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "IF() function must not keep parser in block depth: {stmts:?}"
    );
    assert!(stmts[0].starts_with("SELECT IF("));
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_mysql_if_function_followed_by_case_then_stays_two_statements() {
    let sql = "SELECT IF(score > 90, 'A', 'B') + CASE WHEN bonus > 0 THEN 1 ELSE 0 END AS grade FROM exam_scores;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "IF() function must not treat downstream CASE THEN as IF THEN: {stmts:?}"
    );
    assert!(
        stmts[0].contains("CASE WHEN bonus > 0 THEN 1 ELSE 0 END"),
        "First statement should preserve CASE expression: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_mysql_repeat_function_does_not_open_block_depth() {
    let sql = "WITH RECURSIVE cat_tree AS (\n    SELECT 1 AS lvl\n)\nSELECT REPEAT('  ', lvl) AS indent_prefix FROM cat_tree;\nSELECT 'FINAL_STATUS' AS section_name;";
    let items = QueryExecutor::split_script_items_for_db_type(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
    );
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "REPEAT() function must not keep parser inside a phantom REPEAT block: {stmts:?}"
    );
    assert!(
        stmts[0].contains("SELECT REPEAT('  ', lvl) AS indent_prefix FROM cat_tree"),
        "first statement should preserve REPEAT() scalar function call: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 'FINAL_STATUS' AS section_name"),
        "second statement should remain a separate trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_if_alias_in_case_when_does_not_open_if_block() {
    let sql = "SELECT\n    CASE\n        WHEN if.flag = 'Y' THEN 'YES'\n        ELSE 'NO'\n    END AS flag_text\nFROM qt_if_base if;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "if alias in CASE WHEN must not keep parser inside a phantom IF block: {stmts:?}"
    );
    assert!(
        stmts[0].contains("WHEN if.flag = 'Y' THEN 'YES'"),
        "first statement should preserve CASE expression with if alias: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_mysql_backtick_identifier_with_semicolon_stays_single_statement() {
    let sql = "SELECT `semi;colon` AS c FROM demo;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "semicolon inside MySQL backtick identifier must not split statement: {stmts:?}"
    );
    assert!(
        stmts[0].contains("`semi;colon`"),
        "first statement should preserve backtick identifier: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_mariadb_final_boss_regression() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let items = QueryExecutor::split_script_items(&sql);
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();
    let statements = get_statements(&items);

    assert_eq!(
        tool_command_count, 3,
        "MariaDB final boss script should keep USE + both DELIMITER commands as tool commands: {items:?}"
    );
    assert_eq!(
        statement_count, 23,
        "MariaDB final boss script execution split changed unexpectedly: {items:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("/*!80000 SET @versioned_comment_executed = 1 */")
                && stmt.trim_end().ends_with("*/")
        }),
        "versioned comment statement should stay executable: {statements:?}"
    );
    assert!(
        !statements.iter().any(|stmt| {
            let trimmed = stmt.trim_start();
            trimmed.starts_with('#')
                || (trimmed.starts_with("/*")
                    && !trimmed.starts_with("/*!")
                    && !trimmed.starts_with("/**"))
        }),
        "plain MariaDB comments should not become standalone execution units: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE FUNCTION fn_currency_rate")
                && stmt.contains("WHEN 'KRW' THEN 0.00075")
                && stmt.contains("END")
        }),
        "function body should remain one execution unit: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_final_boss")
                && stmt.contains("read_loop: LOOP")
                && stmt.contains("GET DIAGNOSTICS CONDITION 1")
                && stmt.contains("JSON_TABLE(")
                && stmt.contains("COMMIT")
        }),
        "main stored procedure should remain intact as a single execution unit: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("'PASS' AS status")
                && stmt.contains("expected_duplicate_errors")
        }),
        "post-run verification query should remain independently executable: {statements:?}"
    );
}

#[test]
fn test_split_script_items_mariadb_executable_comment_stays_runnable_statement() {
    let sql = "/*M!100100 SET @feature_flag = 1 */;\nSELECT 1;\n";
    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);

    assert_eq!(
        statements.len(),
        2,
        "MariaDB executable comment should stay executable and not merge with following SQL: {statements:?}"
    );
    assert!(
        statements[0].starts_with("/*M!100100 SET @feature_flag = 1 */"),
        "first statement should preserve MariaDB executable comment: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1");
}

#[test]
fn test_mariadb_test7_select_into_statements_are_not_displayable_results() {
    let sql = include_str!("../../../test_mariadb/test7.txt");
    let items = QueryExecutor::split_script_items(sql);
    let statements = get_statements(&items);
    let select_into_statements = statements
        .iter()
        .copied()
        .filter(|stmt| {
            let upper = stmt.to_ascii_uppercase();
            upper.contains("INTO @")
                && super::mysql_executor::MysqlExecutor::is_select_statement(stmt)
        })
        .collect::<Vec<_>>();

    assert_eq!(
        select_into_statements.len(),
        5,
        "test_mariadb/test7.txt SELECT INTO coverage changed: {select_into_statements:?}"
    );
    for statement in select_into_statements {
        assert!(
            super::mysql_executor::MysqlExecutor::is_select_statement(statement),
            "SELECT INTO remains a MySQL/MariaDB SELECT for execution: {statement}"
        );
        assert!(
            !super::mysql_executor::MysqlExecutor::is_displayable_select_statement(statement),
            "SELECT INTO should not open a Data Grid result tab: {statement}"
        );
    }
}

#[test]
fn test_split_script_items_mariadb_comment_final_boss_keeps_transaction_statement() {
    let sql = load_mariadb_query_test_file("test8.txt");
    assert!(
        !sql.is_empty(),
        "test_mariadb/test8.txt should not be empty"
    );

    let items = QueryExecutor::split_script_items(&sql);

    assert!(
        items.iter().any(
            |item| matches!(item, ScriptItem::Statement(statement) if statement == "START TRANSACTION")
        ),
        "test8 should keep START TRANSACTION as a SQL statement: {items:?}"
    );
    assert!(
        !items.iter().any(|item| matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::RunScript {
                path,
                relative_to_caller: false,
            }) if path.eq_ignore_ascii_case("TRANSACTION")
        )),
        "test8 transaction directive must not be parsed as @TRANSACTION script include: {items:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_emp_table_regression() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let emp_statement_start = sql.find("CREATE TABLE emp").unwrap_or(0);
    let emp_statement_end = sql
        .get(emp_statement_start..)
        .and_then(|tail| tail.find(") ENGINE = InnoDB;"))
        .map(|offset| emp_statement_start + offset + ") ENGINE = InnoDB;".len())
        .unwrap_or(sql.len());
    for cursor in [
        emp_statement_end.saturating_sub(1),
        emp_statement_end.min(sql.len()),
    ] {
        let bounds = QueryExecutor::statement_bounds_at_cursor(&sql, cursor)
            .expect("expected emp CREATE TABLE bounds in MariaDB regression file");
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with("CREATE TABLE emp"),
            "cursor near emp CREATE TABLE tail should resolve emp statement, got:\n{statement}"
        );
        assert!(
            statement.contains("CONSTRAINT chk_emp_salary"),
            "emp statement bounds should keep the full emp DDL, got:\n{statement}"
        );
        assert!(
            !statement.contains("CREATE TABLE dept"),
            "previous dept statement must not leak into emp bounds"
        );
        assert!(
            !statement.contains("CREATE TABLE orders"),
            "next orders statement must not leak into emp bounds"
        );
    }
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_final_boss_procedure_body_regression() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let expected_prefix = "CREATE PROCEDURE sp_run_final_boss ()";
    let anchors = [
        ("handler", "DECLARE CONTINUE HANDLER FOR 1062"),
        (
            "duplicate comment",
            "-- expected duplicate: should be logged by handler, not kill the run",
        ),
        ("utf8 literal", "emoji 😊"),
        ("json_table join", "JOIN JSON_TABLE("),
        ("recursive cte", "WITH RECURSIVE dept_tree AS ("),
        ("window clause", "WINDOW"),
        ("commit", "COMMIT;"),
        ("procedure terminator", "END$$\n\nDELIMITER ;"),
    ];

    for (label, anchor) in anchors {
        let cursor = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected procedure bounds for anchor `{anchor}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with(expected_prefix),
            "cursor at `{label}` should resolve the CREATE PROCEDURE statement, got:\n{statement}"
        );
        assert!(
            statement.contains("CALL sp_assert(v_top_emp_id = 130, 'window ranking mismatch');")
                && statement.contains("COMMIT;"),
            "cursor at `{label}` should keep the full stored procedure body, got:\n{statement}"
        );
        assert!(
            !statement.contains("CALL sp_run_final_boss();"),
            "post-procedure CALL statement must not leak into procedure bounds for anchor `{label}`"
        );
    }
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_end_delimiter_line_resolves_current_routine() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let cases = [
        (
            "function terminator",
            "END$$\n\nCREATE PROCEDURE sp_assert",
            "CREATE FUNCTION fn_currency_rate",
        ),
        (
            "assert procedure terminator",
            "END$$\n\nCREATE TRIGGER trg_orders_bi",
            "CREATE PROCEDURE sp_assert",
        ),
        (
            "before-insert trigger terminator",
            "END$$\n\nCREATE TRIGGER trg_orders_bu",
            "CREATE TRIGGER trg_orders_bi",
        ),
        (
            "main procedure terminator",
            "END$$\n\nDELIMITER ;",
            "CREATE PROCEDURE sp_run_final_boss ()",
        ),
    ];

    for (label, anchor, expected_prefix) in cases {
        let cursor = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected statement bounds for `{label}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with(expected_prefix),
            "cursor at `{label}` should resolve the owning routine, got:\n{statement}"
        );
    }
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_custom_delimiter_gap_prefers_following_routine() {
    let sql = load_mariadb_query_test_file("test3.txt");
    assert!(!sql.is_empty(), "test_mysql/test3.txt should not be empty");

    let cases = [
        (
            "function to assert gap",
            "END$$\n\nCREATE PROCEDURE sp_assert (",
            "CREATE PROCEDURE sp_assert (",
            "CREATE FUNCTION fn_priority_factor",
        ),
        (
            "assert to shift gap",
            "END$$\n\nCREATE PROCEDURE sp_shift_rank (",
            "CREATE PROCEDURE sp_shift_rank (",
            "CREATE PROCEDURE sp_assert (",
        ),
    ];

    for (label, anchor, expected_prefix, previous_prefix) in cases {
        let anchor_start = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let cursor = anchor_start.saturating_add("END$$\n".len());
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected statement bounds for `{label}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with(expected_prefix),
            "cursor in MariaDB custom delimiter gap `{label}` should prefer the following routine, got:\n{statement}"
        );
        assert!(
            !statement.starts_with(previous_prefix),
            "cursor in MariaDB custom delimiter gap `{label}` should not fall back to the previous routine"
        );
    }
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_executable_comment_stays_runnable_statement() {
    let sql = "SET @feature_flag = 0;\n/*!80000 SET @feature_flag = 1 */;\nSELECT 1;\n";
    let cursor = sql.find("/*!80000").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    )
    .expect("expected executable comment bounds");
    let statement = &sql[bounds.0..bounds.1];

    assert_eq!(
        statement.trim(),
        "/*!80000 SET @feature_flag = 1 */",
        "MariaDB executable comment should remain directly executable at cursor: {statement:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_comment_gap_prefers_following_statement() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let cases = [
        (
            "hash comment",
            "# line comment torture: ; ; ; DELIMITER $$ should not matter here",
        ),
        (
            "block comment body",
            "CREATE PROCEDURE fake_proc() BEGIN SELECT 1; END$$",
        ),
    ];

    for (label, anchor) in cases {
        let cursor = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected statement bounds for `{label}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with("CREATE TABLE dept"),
            "cursor in MariaDB comment gap `{label}` should prefer the following runnable statement, got:\n{statement}"
        );
        assert!(
            !statement.starts_with("/*!80000 SET @versioned_comment_executed = 1 */"),
            "cursor in MariaDB comment gap `{label}` should not fall back to the previous executable comment"
        );
    }
}

#[test]
fn test_split_format_items_mariadb_final_boss_regression() {
    let sql = load_mariadb_query_test_file("test1.txt");
    assert!(!sql.is_empty(), "test_mysql/test1.txt should not be empty");

    let items = QueryExecutor::split_format_items(&sql);
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::ToolCommand(_)))
        .count();
    let statements: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(statement) => Some(statement.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        tool_command_count, 3,
        "format split should preserve USE + DELIMITER commands for MariaDB final boss: {items:?}"
    );
    assert_eq!(
        statement_count, 28,
        "format split should preserve executable statements plus standalone comments for MariaDB final boss: {items:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.trim_start()
                .starts_with("/*!80000 SET @versioned_comment_executed = 1 */")
        }),
        "format split should keep MariaDB executable comments as runnable statements: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_final_boss")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR 1062")
                && stmt.contains("ROLLBACK TO SAVEPOINT sp_before_rollback_test")
                && stmt.contains("WINDOW")
                && stmt.contains("END")
        }),
        "format split should keep the main stored procedure as one statement: {statements:?}"
    );
}

#[test]
fn test_split_script_items_mariadb_parser_killer_regression() {
    let sql = load_mariadb_query_test_file("test2.txt");
    assert!(!sql.is_empty(), "test_mysql/test2.txt should not be empty");

    let items = QueryExecutor::split_script_items(&sql);
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();
    let statements = get_statements(&items);

    assert_eq!(
        tool_command_count, 4,
        "MariaDB parser killer script should keep USE + three DELIMITER commands as tool commands: {items:?}"
    );
    assert_eq!(
        statement_count, 25,
        "MariaDB parser killer script execution split changed unexpectedly: {items:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("/*!80000 SET @versioned_comment_ok = 1 */")
                && stmt.trim_end().ends_with("*/")
        }),
        "versioned comment statement should stay executable: {statements:?}"
    );
    assert!(
        !statements.iter().any(|stmt| {
            let trimmed = stmt.trim_start();
            trimmed.starts_with('#')
                || (trimmed.starts_with("/*")
                    && !trimmed.starts_with("/*!")
                    && !trimmed.starts_with("/**"))
        }),
        "plain MariaDB comments should not become standalone execution units: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_parser_killer ()")
                && stmt.contains("DECLARE CONTINUE HANDLER FOR 1062")
                && stmt.contains("JSON_TABLE(")
                && stmt.contains("WITH RECURSIVE node_tree AS (")
                && stmt.contains("ROW_NUMBER() OVER (")
                && stmt.contains("COMMIT")
        }),
        "main stored procedure should remain intact as a single execution unit: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("'PASS' AS status")
                && stmt.contains("expected_duplicate_errors")
        }),
        "post-run verification query should remain independently executable: {statements:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_parser_killer_procedure_body_regression() {
    let sql = load_mariadb_query_test_file("test2.txt");
    assert!(!sql.is_empty(), "test_mysql/test2.txt should not be empty");

    let expected_prefix = "CREATE PROCEDURE sp_run_parser_killer ()";
    let anchors = [
        ("handler", "DECLARE CONTINUE HANDLER FOR 1062"),
        (
            "json literal",
            "contains ; semicolon, DELIMITER //, $$, quote ''x''",
        ),
        ("json_table join", "JOIN JSON_TABLE("),
        ("recursive cte", "WITH RECURSIVE node_tree AS ("),
        ("window order", "ORDER BY weight_sum DESC, owner_name"),
        ("commit", "COMMIT;"),
        ("procedure terminator", "END//\n\nDELIMITER ;"),
    ];

    for (label, anchor) in anchors {
        let cursor = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected procedure bounds for anchor `{anchor}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with(expected_prefix),
            "cursor at `{label}` should resolve the CREATE PROCEDURE statement, got:\n{statement}"
        );
        assert!(
            statement
                .contains("CALL sp_check(v_top_owner = 'alice', 'window ranking owner mismatch');")
                && statement.contains("COMMIT;"),
            "cursor at `{label}` should keep the full stored procedure body, got:\n{statement}"
        );
        assert!(
            !statement.contains("CALL sp_run_parser_killer();"),
            "post-procedure CALL statement must not leak into procedure bounds for anchor `{label}`"
        );
    }
}

#[test]
fn test_split_script_items_mariadb_ultra_final_boss_regression() {
    let sql = load_mariadb_query_test_file("test3.txt");
    assert!(!sql.is_empty(), "test_mysql/test3.txt should not be empty");

    let items = QueryExecutor::split_script_items(&sql);
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();
    let tool_command_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::ToolCommand(_)))
        .count();
    let statements = get_statements(&items);

    assert_eq!(
        tool_command_count, 4,
        "MariaDB ultra final boss script should keep USE + three DELIMITER commands as tool commands: {items:?}"
    );
    assert_eq!(
        statement_count, 25,
        "MariaDB ultra final boss script execution split changed unexpectedly: {items:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("/*!80000 SET @versioned_comment_ok = 1 */")
                && stmt.trim_end().ends_with("*/")
        }),
        "versioned comment statement should stay executable: {statements:?}"
    );
    assert!(
        !statements.iter().any(|stmt| {
            let trimmed = stmt.trim_start();
            trimmed.starts_with('#')
                || (trimmed.starts_with("/*")
                    && !trimmed.starts_with("/*!")
                    && !trimmed.starts_with("/**"))
        }),
        "plain MariaDB comments should not become standalone execution units: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.contains("CREATE PROCEDURE sp_run_ultra_final_boss ()")
                && stmt.contains("CALL sp_assert(v_local = 10, 'nested block / CASE mismatch');")
                && stmt.contains("JOIN JSON_TABLE(")
                && stmt.contains("WINDOW")
                && stmt.contains("CALL sp_assert(v_top_run_id = 4001, 'top run id mismatch');")
                && stmt.contains("COMMIT")
        }),
        "main stored procedure should remain intact as a single execution unit: {statements:?}"
    );
    assert!(
        statements.iter().any(|stmt| {
            stmt.starts_with("SELECT")
                && stmt.contains("'PASS' AS status")
                && stmt.contains("top_run_id")
        }),
        "post-run verification query should remain independently executable: {statements:?}"
    );
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_ultra_final_boss_procedure_body_regression() {
    let sql = load_mariadb_query_test_file("test3.txt");
    assert!(!sql.is_empty(), "test_mysql/test3.txt should not be empty");

    let expected_prefix = "CREATE PROCEDURE sp_run_ultra_final_boss ()";
    let anchors = [
        (
            "routine comment torture",
            "-- comment torture inside routine ; END ; DELIMITER // $$ should not matter",
        ),
        (
            "json literal",
            "token DELIMITER $$, token //, emoji 🤖, 한글",
        ),
        ("json_table join", "JOIN JSON_TABLE("),
        ("window clause", "WINDOW"),
        (
            "global rank order",
            "ORDER BY s.weighted_minutes DESC, s.owner_name, s.run_id",
        ),
        (
            "top run id",
            "CALL sp_assert(v_top_run_id = 4001, 'top run id mismatch');",
        ),
        ("procedure terminator", "END//\n\nDELIMITER ;"),
    ];

    for (label, anchor) in anchors {
        let cursor = sql
            .find(anchor)
            .unwrap_or_else(|| panic!("expected anchor `{anchor}` in MariaDB regression file"));
        let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(
            &sql,
            cursor,
            Some(crate::db::connection::DatabaseType::MySQL),
        )
        .unwrap_or_else(|| panic!("expected procedure bounds for anchor `{anchor}`"));
        let statement = &sql[bounds.0..bounds.1];

        assert!(
            statement.starts_with(expected_prefix),
            "cursor at `{label}` should resolve the CREATE PROCEDURE statement, got:\n{statement}"
        );
        assert!(
            statement.contains("CALL sp_assert(v_top_owner = 'alice', 'top owner mismatch');")
                && statement
                    .contains("CALL sp_assert(v_top_run_id = 4001, 'top run id mismatch');")
                && statement.contains("COMMIT;"),
            "cursor at `{label}` should keep the full stored procedure body, got:\n{statement}"
        );
        assert!(
            !statement.contains("CALL sp_run_ultra_final_boss();"),
            "post-procedure CALL statement must not leak into procedure bounds for anchor `{label}`"
        );
    }
}

#[test]
fn test_split_format_items_mysql_if_function_followed_by_case_then_stays_two_statements() {
    let sql = "SELECT IF(score > 90, 'A', 'B') + CASE WHEN bonus > 0 THEN 1 ELSE 0 END AS grade FROM exam_scores;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must match split_script_items for IF() + CASE THEN inputs: {stmts:?}"
    );
    assert!(
        stmts[0].contains("CASE WHEN bonus > 0 THEN 1 ELSE 0 END"),
        "First formatted statement should preserve CASE expression: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_line_block_depths_if_function_line_stays_top_level_without_then() {
    let sql = "SELECT IF(score > 90, 'A', 'B') AS grade\nFROM exam_scores;";
    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(
        depths,
        vec![0, 0],
        "IF() scalar function should not affect block depth"
    );
}

#[test]
fn test_line_block_depths_ignores_procedure_keyword_in_comment_for_begin_prededent() {
    let sql = "BEGIN\n  -- PROCEDURE marker in comment\n  BEGIN\n    NULL;\n  END;\nEND;";
    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(
        depths,
        vec![0, 1, 1, 2, 1, 0],
        "Comment text must not trigger subprogram BEGIN pre-dedent"
    );
}

#[test]
fn test_line_block_depths_ignores_function_keyword_in_string_for_begin_prededent() {
    let sql = "BEGIN\n  v_sql := 'FUNCTION marker in string';\n  BEGIN\n    NULL;\n  END;\nEND;";
    let depths = QueryExecutor::line_block_depths(sql);

    assert_eq!(
        depths,
        vec![0, 1, 1, 2, 1, 0],
        "String literal text must not trigger subprogram BEGIN pre-dedent"
    );
}

#[test]
fn test_line_block_depths_preserves_subquery_depth_after_non_subquery_parentheses() {
    let sql = "SELECT *\nFROM (\n  SELECT (1 + 2) AS n\n  FROM dual\n) q;";
    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let from_open_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM ("))
        .expect("expected FROM ( line");
    let nested_select_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .to_uppercase()
                .starts_with("SELECT (1 + 2)")
        })
        .expect("expected nested SELECT line");
    let nested_from_idx = lines
        .iter()
        .position(|line| line.trim_start().to_uppercase().starts_with("FROM DUAL"))
        .expect("expected nested FROM line");

    assert!(
        depths[nested_select_idx] > depths[from_open_idx],
        "Nested SELECT should be deeper than outer FROM (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[nested_from_idx], depths[nested_select_idx],
        "Non-subquery parentheses inside nested SELECT must not prematurely dedent subquery depth (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_package_body_mixed_end_suffix_and_named_end() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth AS
  PROCEDURE p1 IS
  BEGIN
    IF 1 = 1 THEN
      NULL;
    ELSE
      NULL;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END p1;
END pkg_depth;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let if_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("IF 1 = 1 THEN"))
        .expect("expected IF line");
    let else_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("ELSE"))
        .expect("expected ELSE line");
    let end_if_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END IF"))
        .expect("expected END IF line");
    let exception_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("EXCEPTION"))
        .expect("expected EXCEPTION line");
    let when_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("WHEN OTHERS THEN"))
        .expect("expected WHEN OTHERS line");
    let end_proc_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END p1"))
        .expect("expected END p1 line");
    let end_pkg_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END pkg_depth"))
        .expect("expected END pkg_depth line");

    assert_eq!(
        depths[end_if_idx], depths[if_idx],
        "END IF should align with IF in nested package body blocks (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[else_idx], depths[if_idx],
        "ELSE should align with IF in nested package body blocks (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[exception_idx], depths[end_proc_idx],
        "EXCEPTION should align with END <subprogram_name> level (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[when_idx],
        depths[exception_idx].saturating_add(1),
        "WHEN branch should be one level deeper than EXCEPTION (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[end_pkg_idx], 0,
        "named package body END should dedent to top-level (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_package_body_handles_end_name_with_inline_comment() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth_comment AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1; -- procedure end
END pkg_depth_comment; -- package end"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let proc_header_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PROCEDURE p1 IS"))
        .expect("expected PROCEDURE header line");
    let end_proc_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END p1"))
        .expect("expected END p1 line");
    let end_pkg_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END pkg_depth_comment"))
        .expect("expected END package line");

    assert_eq!(
        depths[end_proc_idx], depths[proc_header_idx],
        "named subprogram END with inline comments should keep procedure depth (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[end_pkg_idx], 0,
        "named package END with inline comments should dedent to top-level (depths: {:?})",
        depths
    );
}

#[test]
fn test_line_block_depths_package_body_quoted_end_label_does_not_trigger_end_suffix_dedent() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth_q AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END "IF";
END pkg_depth_q;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let proc_header_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PROCEDURE p1 IS"))
        .expect("expected PROCEDURE header line");
    let end_proc_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END \"IF\""))
        .expect("expected quoted END label line");
    let end_pkg_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END pkg_depth_q"))
        .expect("expected package END line");

    assert_eq!(
        depths[end_proc_idx], depths[proc_header_idx],
        "quoted END labels (END \"IF\") must align with subprogram header depth (depths: {:?})",
        depths
    );
    assert_eq!(
        depths[end_pkg_idx], 0,
        "package END should still dedent to top-level after quoted END label (depths: {:?})",
        depths
    );
}

#[test]
fn test_split_script_items_package_body_with_is_if_else_exception_and_named_end_stays_single_statement(
) {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth_mix IS
  PROCEDURE p1 AS
  BEGIN
    IF 1 = 1 THEN
      NULL;
    ELSE
      NULL;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END p1;
END pkg_depth_mix;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "package body should remain single statement across IS/AS, IF/ELSE, EXCEPTION, named END: {stmts:?}"
    );
    assert!(stmts[0].contains("END IF;"));
    assert!(stmts[0].contains("EXCEPTION"));
    assert!(stmts[0].contains("END p1;"));
    assert!(stmts[0].contains("END pkg_depth_mix"));
}

#[test]
fn test_line_block_depths_package_body_end_name_on_next_line_after_nested_end_if() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth_split IS
  PROCEDURE p1 IS
  BEGIN
    IF 1 = 1 THEN
      NULL;
    END
    IF;
  END
  p1;
END
pkg_depth_split;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let end_if_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END")
        .expect("expected split END for IF");
    let if_suffix_idx = end_if_idx + 1;
    let package_name_suffix_idx = lines
        .iter()
        .position(|line| line.trim_start() == "pkg_depth_split;")
        .expect("expected split package END name line");

    assert_eq!(
        depths[if_suffix_idx], depths[end_if_idx],
        "split END/IF should preserve the same dedented depth (depths: {depths:?})"
    );
    assert_eq!(
        depths[package_name_suffix_idx], 0,
        "split named package END should dedent to top-level (depths: {depths:?})"
    );
}

#[test]
fn test_line_block_depths_package_body_named_member_end_followed_by_unlabeled_package_end() {
    let sql = r#"CREATE PACKAGE BODY a IS
    PROCEDURE b (c IN VARCHAR2) IS
    BEGIN
        IF (1 = 1) THEN
            BEGIN
                INSERT INTO d (e)
                VALUES (1);
            END;
        END IF;
        OPEN v FOR
            SELECT 1
            FROM dual;
    END b;
END;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let procedure_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PROCEDURE b"))
        .expect("expected PROCEDURE line");
    let end_proc_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END b;")
        .expect("expected END b line");
    let end_pkg_idx = lines
        .iter()
        .rposition(|line| line.trim_start() == "END;")
        .expect("expected final END line");
    let select_idx = lines
        .iter()
        .rposition(|line| line.trim_start() == "SELECT 1 FROM dual;")
        .expect("expected trailing SELECT line");

    assert_eq!(
        depths[end_proc_idx], depths[procedure_idx],
        "named subprogram END should close back to the package member depth (depths: {depths:?})"
    );
    assert_eq!(
        depths[end_pkg_idx], 0,
        "final unlabeled package END should return to top-level depth (depths: {depths:?})"
    );
    assert_eq!(
        depths[select_idx], 0,
        "trailing SELECT should start at top-level after package body closure (depths: {depths:?})"
    );
}

#[test]
fn test_split_script_items_package_body_named_member_end_before_unlabeled_package_end_stays_single_statement(
) {
    let sql = r#"CREATE PACKAGE BODY a IS
    PROCEDURE b (c IN VARCHAR2) IS
    BEGIN
        IF (1 = 1) THEN
            BEGIN
                INSERT INTO d (e)
                VALUES (1);
            END;
        END IF;
        OPEN v FOR
            SELECT 1
            FROM dual;
    END b;
END;
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "package body should split only at the final unlabeled END: {stmts:?}"
    );
    assert!(stmts[0].contains("END IF;"));
    assert!(stmts[0].contains("END b;"));
    assert!(
        stmts[0].trim_end().ends_with("END"),
        "first statement should keep the final package END line, got: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn test_split_script_items_package_body_split_end_name_with_exception_and_if_suffix() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_depth_split2 AS
  PROCEDURE p1 IS
  BEGIN
    IF 1 = 1 THEN
      NULL;
    END
    IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END
  p1;
END
pkg_depth_split2;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "split END labels in package body should not swallow trailing statement: {stmts:?}"
    );
    let first_upper = stmts[0].to_ascii_uppercase();
    assert!(first_upper.contains(
        "END
    IF;"
    ));
    assert!(first_upper.contains(
        "END
  P1;"
    ));
    assert!(
        first_upper.contains("END") && first_upper.contains("PKG_DEPTH_SPLIT2"),
        "first statement missing split package end label: {}",
        stmts[0]
    );
    assert!(stmts[1]
        .to_ascii_uppercase()
        .starts_with("SELECT 1 FROM DUAL"));
}

#[test]
fn test_line_block_depths_package_body_named_end_starting_with_end_suffix_keyword() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if_pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1;
END IF_PKG;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let end_pkg_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END IF_PKG"))
        .expect("expected END IF_PKG line");
    let select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1 FROM dual"))
        .expect("expected trailing SELECT line");

    assert_eq!(
        depths[end_pkg_idx], 0,
        "named package END should stay at top-level when label starts with IF (depths: {depths:?})"
    );
    assert_eq!(
        depths[select_idx], 0,
        "depth should fully reset after package END with IF-prefixed label (depths: {depths:?})"
    );
}

#[test]
fn test_split_script_items_package_body_named_end_starting_with_end_suffix_keyword() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if_pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1;
END IF_PKG;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "package END IF_PKG should not keep following SELECT in same statement: {stmts:?}"
    );
    assert!(
        stmts[0].to_ascii_uppercase().contains("END IF_PKG"),
        "first statement should keep named package END: {}",
        stmts[0]
    );
    assert!(stmts[1]
        .to_ascii_uppercase()
        .starts_with("SELECT 1 FROM DUAL"));
}

#[test]
fn test_line_block_depths_package_body_schema_qualified_end_starting_with_end_suffix_keyword() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if_pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1;
END IF_OWNER.IF_PKG;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let end_pkg_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END IF_OWNER.IF_PKG"))
        .expect("expected END IF_OWNER.IF_PKG line");
    let select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1 FROM dual"))
        .expect("expected trailing SELECT line");

    assert_eq!(
        depths[end_pkg_idx], 0,
        "schema-qualified package END should stay at top-level even when owner starts with IF (depths: {depths:?})"
    );
    assert_eq!(
        depths[select_idx], 0,
        "depth should fully reset after schema-qualified package END with IF-prefixed owner (depths: {depths:?})"
    );
}

#[test]
fn test_line_block_depths_package_body_split_schema_qualified_end_label_continuation() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if_pkg AS
  PROCEDURE p1 IS
  BEGIN
    IF x THEN
      NULL;
    END IF;
  END p1;
END
IF_OWNER.IF_PKG;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let end_line_idx = lines
        .iter()
        .position(|line| line.trim_start() == "END")
        .expect("expected split END line");
    let label_line_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("IF_OWNER.IF_PKG"))
        .expect("expected schema-qualified END label continuation line");
    let select_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("SELECT 1 FROM dual"))
        .expect("expected trailing SELECT line");

    assert_eq!(
        depths[end_line_idx], 0,
        "split package END line should pre-dedent to top-level (depths: {depths:?})"
    );
    assert_eq!(
        depths[label_line_idx], 0,
        "schema-qualified END label continuation should remain at top-level (depths: {depths:?})"
    );
    assert_eq!(
        depths[select_idx], 0,
        "depth should be reset after split schema-qualified package END (depths: {depths:?})"
    );
}

#[test]
fn test_split_script_items_package_body_schema_qualified_end_starting_with_end_suffix_keyword() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if_pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1;
END IF_OWNER.IF_PKG;
/
SELECT 1 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "schema-qualified package END with IF-prefixed owner should not swallow trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0]
            .to_ascii_uppercase()
            .contains("END IF_OWNER.IF_PKG"),
        "first statement should preserve schema-qualified package END: {}",
        stmts[0]
    );
    assert!(stmts[1]
        .to_ascii_uppercase()
        .starts_with("SELECT 1 FROM DUAL"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_single_statement_until_main_select() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nSELECT f() FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration must stay attached to main SELECT statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f() FROM dual"),
        "first statement should include main SELECT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_and_search_cycle_keeps_single_statement() {
    let sql = "WITH\n  FUNCTION f(p_id NUMBER) RETURN NUMBER IS\n  BEGIN\n    RETURN p_id;\n  END;\n  t (id, parent_id) AS (\n    SELECT 1, NULL FROM dual\n    UNION ALL\n    SELECT id + 1, id FROM t WHERE id < 3\n  )\nSEARCH DEPTH FIRST BY id SET order_col\nCYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'\nSELECT f(id), parent_id FROM t;\nSELECT 2 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION + SEARCH/CYCLE should remain a single statement before trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].contains("FUNCTION f(p_id NUMBER) RETURN NUMBER IS"),
        "first statement should keep WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SEARCH DEPTH FIRST BY id SET order_col"),
        "SEARCH clause should remain in the same statement: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("CYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'"),
        "CYCLE clause should remain in the same statement: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f(id), parent_id FROM t"),
        "main SELECT should remain in first statement: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_and_search_cycle_keeps_single_statement() {
    let sql = "WITH\n  FUNCTION f(p_id NUMBER) RETURN NUMBER IS\n  BEGIN\n    RETURN p_id;\n  END;\n  t (id, parent_id) AS (\n    SELECT 1, NULL FROM dual\n    UNION ALL\n    SELECT id + 1, id FROM t WHERE id < 3\n  )\nSEARCH DEPTH FIRST BY id SET order_col\nCYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'\nSELECT f(id), parent_id FROM t;\nSELECT 2 FROM dual;";

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION + SEARCH/CYCLE should remain a single format statement before trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].contains("FUNCTION f(p_id NUMBER) RETURN NUMBER IS"),
        "first statement should keep WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SEARCH DEPTH FIRST BY id SET order_col"),
        "SEARCH clause should remain in the same format statement: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("CYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'"),
        "CYCLE clause should remain in the same format statement: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f(id), parent_id FROM t"),
        "main SELECT should remain in first format statement: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_as_keeps_single_statement_until_main_select() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER AS
  BEGIN
    RETURN 1;
  END;
SELECT f() FROM dual;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION ... AS declaration must stay attached to main SELECT statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER AS"
        ),
        "first statement should preserve WITH FUNCTION AS declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f() FROM dual"),
        "first statement should include main SELECT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_procedure_keeps_single_statement_until_main_select() {
    let sql = "WITH\n  PROCEDURE p IS\n  BEGIN\n    NULL;\n  END;\nSELECT 1 FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must keep WITH PROCEDURE declaration with its main SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  PROCEDURE p IS"),
        "first formatted statement should preserve WITH PROCEDURE declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT 1 FROM dual"),
        "first formatted statement should include main SELECT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_single_statement_until_main_merge() {
    let sql = "WITH\n  FUNCTION normalize_id(p_id NUMBER) RETURN NUMBER IS\n  BEGIN\n    RETURN p_id;\n  END;\nMERGE INTO target t\nUSING (SELECT normalize_id(1) AS id FROM dual) s\nON (t.id = s.id)\nWHEN MATCHED THEN\n  UPDATE SET t.id = s.id;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration must stay attached to main MERGE statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION normalize_id"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("MERGE INTO target t"),
        "first statement should include main MERGE: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_keeps_single_statement_until_main_insert() {
    let sql = "WITH\n  PROCEDURE p_log(p_id NUMBER) IS\n  BEGIN\n    NULL;\n  END;\nINSERT INTO target(id)\nSELECT 1 FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH PROCEDURE declaration must stay attached to main INSERT statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  PROCEDURE p_log"),
        "first statement should preserve WITH PROCEDURE declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("INSERT INTO target(id)"),
        "first statement should include main INSERT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_single_statement_until_main_update() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nUPDATE target\nSET id = f()\nWHERE id = 1;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration must stay attached to main UPDATE statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("UPDATE target"),
        "first statement should include main UPDATE: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_keeps_single_statement_until_main_delete() {
    let sql = "WITH\n  PROCEDURE p_noop IS\n  BEGIN\n    NULL;\n  END;\nDELETE FROM target\nWHERE id IN (SELECT 1 FROM dual);\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH PROCEDURE declaration must stay attached to main DELETE statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  PROCEDURE p_noop IS"),
        "first statement should preserve WITH PROCEDURE declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("DELETE FROM target"),
        "first statement should include main DELETE: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_single_statement_until_main_values() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nVALUES (f());\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration must stay attached to main VALUES statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("VALUES (f())"),
        "first statement should include main VALUES body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_single_statement_until_main_call() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
CALL consume_fn(f());
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration must stay attached to main CALL statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("CALL consume_fn(f())"),
        "first statement should include main CALL body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_without_semicolon_uses_slash_terminator() {
    let sql = "WITH PROCEDURE p IS\nBEGIN\n  NULL;\nEND\n/\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "expected WITH PROCEDURE declaration and trailing SELECT split, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH PROCEDURE p IS"),
        "first statement should preserve WITH PROCEDURE block, got: {}",
        stmts[0]
    );
    assert!(stmts[0].contains("END"));
    assert_eq!(stmts[1], "SELECT 2 FROM dual");
}

#[test]
fn test_split_script_items_oracle_with_function_without_semicolon_recovers_to_create_statement_head(
) {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END
CREATE TABLE t_parser_recover_no_semicolon (id NUMBER);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode without END semicolon when CREATE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("CREATE TABLE t_parser_recover_no_semicolon"),
        "second statement should start at CREATE TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_without_semicolon_recovers_to_alter_statement_head(
) {
    let sql = "WITH
  PROCEDURE p IS
  BEGIN
    NULL;
  END
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD';
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH PROCEDURE declaration mode without END semicolon when ALTER starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  PROCEDURE p IS"
        ),
        "first statement should preserve WITH PROCEDURE declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("ALTER SESSION SET NLS_DATE_FORMAT"),
        "second statement should start at ALTER SESSION after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_create_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
CREATE TABLE t_parser_recover (id NUMBER);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when CREATE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("CREATE TABLE t_parser_recover"),
        "second statement should start at CREATE TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_recovers_to_alter_statement_head() {
    let sql = "WITH
  PROCEDURE p IS
  BEGIN
    NULL;
  END;
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD';
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH PROCEDURE declaration mode when ALTER starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  PROCEDURE p IS"
        ),
        "first statement should preserve WITH PROCEDURE declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("ALTER SESSION SET NLS_DATE_FORMAT"),
        "second statement should start at ALTER SESSION after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_declare_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
DECLARE
  v NUMBER := 1;
BEGIN
  NULL;
END;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when DECLARE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("DECLARE\n  v NUMBER := 1;"),
        "second statement should start at DECLARE block after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_begin_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
BEGIN
  NULL;
END;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when BEGIN starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].contains(
            "BEGIN
  NULL;
END"
        ),
        "second statement should start at BEGIN block after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_procedure_recovers_to_drop_statement_head() {
    let sql = "WITH
  PROCEDURE p IS
  BEGIN
    NULL;
  END;
DROP TABLE t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH PROCEDURE declaration mode when DROP starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  PROCEDURE p IS"),
        "first statement should preserve WITH PROCEDURE declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("DROP TABLE t_parser_recover"),
        "second statement should start at DROP statement after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_savepoint_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
SAVEPOINT before_batch;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when SAVEPOINT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SAVEPOINT before_batch"),
        "second statement should start at SAVEPOINT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_lock_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
LOCK TABLE t_parser_recover IN EXCLUSIVE MODE;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when LOCK starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("LOCK TABLE t_parser_recover IN EXCLUSIVE MODE"),
        "second statement should start at LOCK TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_incomplete_create_function_recovers_to_values_statement_head() {
    let sql = "CREATE OR REPLACE FUNCTION f RETURN NUMBER IS
BEGIN
  RETURN 1;
END
VALUES (42);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover incomplete CREATE FUNCTION when VALUES starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve incomplete CREATE FUNCTION body: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("VALUES (42)"),
        "second statement should start at VALUES after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_incomplete_create_function_recovers_to_table_statement_head() {
    let sql = "CREATE OR REPLACE FUNCTION f RETURN NUMBER IS
BEGIN
  RETURN 1;
END
TABLE t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover incomplete CREATE FUNCTION when TABLE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve incomplete CREATE FUNCTION body: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("TABLE t_parser_recover"),
        "second statement should start at TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_disassociate_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
DISASSOCIATE STATISTICS FROM TABLES t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when DISASSOCIATE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("DISASSOCIATE STATISTICS FROM TABLES t_parser_recover"),
        "second statement should start at DISASSOCIATE statement after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_associate_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
ASSOCIATE STATISTICS WITH TABLES t_parser_recover DEFAULT COST (10, 20, 30);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when ASSOCIATE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with(
            "ASSOCIATE STATISTICS WITH TABLES t_parser_recover DEFAULT COST (10, 20, 30)"
        ),
        "second statement should start at ASSOCIATE statement after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_purge_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
PURGE TABLE t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when PURGE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("PURGE TABLE t_parser_recover"),
        "second statement should start at PURGE TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_flashback_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
FLASHBACK TABLE t_parser_recover TO BEFORE DROP;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when FLASHBACK starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("FLASHBACK TABLE t_parser_recover TO BEFORE DROP"),
        "second statement should start at FLASHBACK TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_audit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
AUDIT SESSION;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when AUDIT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("AUDIT SESSION"),
        "second statement should start at AUDIT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_noaudit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
NOAUDIT SESSION;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when NOAUDIT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("NOAUDIT SESSION"),
        "second statement should start at NOAUDIT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_lock_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
LOCK TABLE t_parser_recover IN EXCLUSIVE MODE;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when LOCK starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("LOCK TABLE t_parser_recover IN EXCLUSIVE MODE"),
        "second formatted statement should start at LOCK TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_disassociate_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
DISASSOCIATE STATISTICS FROM TABLES t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode at DISASSOCIATE statement head: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("DISASSOCIATE STATISTICS FROM TABLES t_parser_recover"),
        "second formatted statement should start at DISASSOCIATE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_associate_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
ASSOCIATE STATISTICS WITH TABLES t_parser_recover DEFAULT COST (10, 20, 30);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode at ASSOCIATE statement head: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with(
            "ASSOCIATE STATISTICS WITH TABLES t_parser_recover DEFAULT COST (10, 20, 30)"
        ),
        "second formatted statement should start at ASSOCIATE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_purge_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
PURGE TABLE t_parser_recover;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when PURGE starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("PURGE TABLE t_parser_recover"),
        "second formatted statement should start at PURGE TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_flashback_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
FLASHBACK TABLE t_parser_recover TO BEFORE DROP;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when FLASHBACK starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("FLASHBACK TABLE t_parser_recover TO BEFORE DROP"),
        "second formatted statement should start at FLASHBACK TABLE after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_audit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
AUDIT SESSION;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when AUDIT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("AUDIT SESSION"),
        "second formatted statement should start at AUDIT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_comment_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
COMMENT ON TABLE t_parser_recover IS 'recovered';
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when COMMENT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("COMMENT ON TABLE t_parser_recover IS 'recovered'"),
        "second statement should start at COMMENT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_rename_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
RENAME t_parser_recover TO t_parser_recover_new;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        3,
        "parser should recover WITH FUNCTION declaration mode when RENAME starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("RENAME t_parser_recover TO t_parser_recover_new"),
        "second statement should start at RENAME after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_comment_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
COMMENT ON TABLE t_parser_recover IS 'recovered';
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when COMMENT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("COMMENT ON TABLE t_parser_recover IS 'recovered'"),
        "second formatted statement should start at COMMENT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_rename_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
RENAME t_parser_recover TO t_parser_recover_new;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when RENAME starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("RENAME t_parser_recover TO t_parser_recover_new"),
        "second formatted statement should start at RENAME after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}
#[test]
fn test_split_format_items_oracle_with_function_recovers_to_noaudit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
NOAUDIT SESSION;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        3,
        "split_format_items should recover WITH FUNCTION declaration mode when NOAUDIT starts a new statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should preserve WITH FUNCTION declaration block: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("NOAUDIT SESSION"),
        "second formatted statement should start at NOAUDIT after recovery: {}",
        stmts[1]
    );
    assert!(stmts[2].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_and_cte_keeps_single_statement() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
  cte AS (
    SELECT f() AS n FROM dual
  )
SELECT n FROM cte;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION declaration + CTE must stay attached to main SELECT statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("cte AS ("),
        "first statement should keep the CTE clause after function declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT n FROM cte"),
        "first statement should include the main SELECT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_end_label_and_trailing_ctes_keep_single_statement()
{
    let sql = "WITH
    FUNCTION calc_depth (p_id NUMBER) RETURN NUMBER IS v_depth NUMBER;

BEGIN
    SELECT MAX (LEVEL)
    INTO v_depth
    FROM org_tree
    START WITH parent_id IS NULL
    CONNECT BY PRIOR node_id = parent_id;
    RETURN v_depth;
END calc_depth;

recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS (
    SELECT node_id,
        parent_id,
        node_name,
        1 AS DEPTH,
        CAST (node_name AS VARCHAR2 (4000)) AS PATH
    FROM org_tree
    WHERE parent_id IS NULL
    UNION ALL
    SELECT t.node_id,
        t.parent_id,
        t.node_name,
        rt.DEPTH + 1,
        rt.PATH || ' > ' || t.node_name
    FROM org_tree t
    JOIN recursive_tree rt
        ON t.parent_id = rt.node_id
    WHERE rt.DEPTH < calc_depth (t.node_id)
),
    aggregated AS (
        SELECT parent_id,
            COUNT (*) AS child_count,
            MAX (DEPTH) AS max_depth,
            LISTAGG (node_name, ', ') WITHIN GROUP (ORDER BY node_name) AS children
        FROM recursive_tree
        WHERE DEPTH > 1
        GROUP BY parent_id
    )
SELECT rt.*,
    a.child_count,
    a.max_depth,
    a.children
FROM recursive_tree rt
LEFT JOIN aggregated a
    ON rt.node_id = a.parent_id
ORDER BY rt.PATH;
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION end label + trailing CTEs must remain a single statement: {stmts:?}"
    );
    assert!(
        stmts[0].contains("END calc_depth;"),
        "first statement should preserve the labeled function END: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS"),
        "first statement should preserve the first trailing CTE: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("aggregated AS ("),
        "first statement should preserve subsequent trailing CTEs: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("ORDER BY rt.PATH"),
        "first statement should include the main SELECT tail: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_sql_macro_table_keeps_single_statement() {
    let sql = "WITH
  FUNCTION f(p_owner VARCHAR2)
    RETURN VARCHAR2 SQL_MACRO(TABLE)
  IS
  BEGIN
    RETURN 'SELECT owner FROM all_objects WHERE owner = p_owner';
  END;
SELECT * FROM f('SYS');
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION SQL_MACRO(TABLE) declaration should stay attached to its main SELECT statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f(p_owner VARCHAR2)"),
        "first statement should preserve WITH FUNCTION SQL macro declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("RETURN VARCHAR2 SQL_MACRO(TABLE)"),
        "first statement should keep SQL_MACRO(TABLE) signature inside declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT * FROM f('SYS')"),
        "first statement should include main SELECT after declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_sql_macro_table_keeps_single_statement() {
    let sql = "WITH
  FUNCTION f(p_owner VARCHAR2)
    RETURN VARCHAR2 SQL_MACRO(TABLE)
  IS
  BEGIN
    RETURN 'SELECT owner FROM all_objects WHERE owner = p_owner';
  END;
SELECT * FROM f('SYS');
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep WITH FUNCTION SQL_MACRO(TABLE) declaration attached to main SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH\n  FUNCTION f(p_owner VARCHAR2)"),
        "first formatted statement should preserve WITH FUNCTION SQL macro declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("RETURN VARCHAR2 SQL_MACRO(TABLE)"),
        "first formatted statement should keep SQL_MACRO(TABLE) signature inside declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT * FROM f('SYS')"),
        "first formatted statement should include main SELECT after declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_create_type_opaque_keeps_single_statement() {
    let sql = "CREATE OR REPLACE TYPE t_opaque AS OPAQUE (
  STORAGE RAW(16)
);
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE TYPE ... AS OPAQUE should split at the type terminator: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE t_opaque AS OPAQUE"),
        "first statement should preserve TYPE OPAQUE declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_create_type_json_keeps_single_statement() {
    let sql = "CREATE OR REPLACE TYPE t_json AS JSON\n(\n  STRICT\n);\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE TYPE ... AS JSON should split at the type terminator: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE t_json AS JSON"),
        "first statement should preserve TYPE JSON declaration: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_create_type_body_with_member_function_keeps_single_statement() {
    let sql = "CREATE OR REPLACE TYPE BODY t_demo AS\n  MEMBER FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nEND;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE TYPE BODY with member function should remain a single statement until final END: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TYPE BODY t_demo AS"),
        "first statement should preserve TYPE BODY header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("MEMBER FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve member function body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_create_java_source_keeps_body_until_slash() {
    let sql = r#"CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED "DemoClass" AS
public class DemoClass {
  public static String hello() {
    return "hello";
  }
}
/
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE JAVA SOURCE should keep Java body semicolons inside one statement until slash delimiter: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED \"DemoClass\" AS"),
        "first statement should preserve JAVA SOURCE header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("return \"hello\";"),
        "first statement should preserve Java body semicolon: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_create_java_source_keeps_body_until_slash() {
    let sql = r#"CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED "DemoClass" AS
public class DemoClass {
  public static String hello() {
    return "hello";
  }
}
/
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        items.iter().any(|item| matches!(item, FormatItem::Slash)),
        "CREATE JAVA SOURCE should keep SQL*Plus slash delimiter in format items"
    );
    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep JAVA SOURCE body as one statement and split trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE AND COMPILE JAVA SOURCE NAMED \"DemoClass\" AS"),
        "first formatted statement should preserve JAVA SOURCE header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("return \"hello\";"),
        "first formatted statement should preserve Java body semicolon: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_create_wrapped_keeps_body_until_slash() {
    let sql = "CREATE OR REPLACE PROCEDURE wrapped_demo
WRAPPED
a000000
1
abcd;
efgh;
/
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE ... WRAPPED should keep wrapped body semicolons inside one statement until slash delimiter: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "CREATE OR REPLACE PROCEDURE wrapped_demo
WRAPPED"
        ),
        "first statement should preserve WRAPPED header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains(
            "abcd;
efgh;"
        ),
        "first statement should preserve wrapped body with internal semicolons: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_create_wrapped_keeps_body_until_slash() {
    let sql = "CREATE OR REPLACE PROCEDURE wrapped_demo
WRAPPED
a000000
1
abcd;
efgh;
/
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        items.iter().any(|item| matches!(item, FormatItem::Slash)),
        "CREATE WRAPPED should keep SQL*Plus slash delimiter in format items"
    );
    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep WRAPPED body as one statement and split trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with(
            "CREATE OR REPLACE PROCEDURE wrapped_demo
WRAPPED"
        ),
        "first formatted statement should preserve WRAPPED header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains(
            "abcd;
efgh;"
        ),
        "first formatted statement should preserve wrapped body semicolons: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_cte_using_function_call_splits_normally() {
    let sql =
        "WITH cte AS (SELECT 1 AS n FROM dual)\nSELECT ABS(n) AS v FROM cte;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "regular CTE with scalar FUNCTION call should split on first statement terminator: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH cte AS (SELECT 1 AS n FROM dual)"),
        "first statement should preserve CTE query: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT ABS(n) AS v FROM cte"),
        "first statement should include scalar function call in SELECT list: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_nested_with_function_subquery_splits_normally() {
    let sql = "SELECT *\nFROM (\n  WITH\n    FUNCTION inner_f RETURN NUMBER IS\n    BEGIN\n      RETURN 1;\n    END;\n  SELECT inner_f() AS v FROM dual\n) t;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "WITH FUNCTION inside subquery must not suppress top-level split: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("SELECT *\nFROM (\n  WITH\n    FUNCTION inner_f RETURN NUMBER IS"),
        "first statement should preserve nested WITH FUNCTION subquery: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should start with trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_oracle_parenthesized_with_function_cte_splits_normally() {
    let sql = "WITH outer_cte AS (\n  WITH\n    FUNCTION inner_f RETURN NUMBER IS\n    BEGIN\n      RETURN 1;\n    END;\n  SELECT inner_f() AS v FROM dual\n)\nSELECT * FROM outer_cte;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "parenthesized nested WITH FUNCTION CTE must still split at top-level semicolon: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("WITH outer_cte AS (\n  WITH\n    FUNCTION inner_f RETURN NUMBER IS"),
        "first statement should preserve nested WITH FUNCTION CTE: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT * FROM outer_cte"),
        "first statement should include main outer SELECT: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_set_transaction_is_not_sqlplus_set_tool_command() {
    let sql = "SET TRANSACTION READ ONLY;
SELECT 2 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec![
            "SET TRANSACTION READ ONLY".to_string(),
            "SELECT 2 FROM dual".to_string()
        ],
        "SET TRANSACTION should be parsed as SQL statement, not SQL*Plus SET tool command: {items:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !matches!(item, ScriptItem::ToolCommand(_))),
        "SET TRANSACTION must not produce tool command items: {items:?}"
    );
}

#[test]
fn test_split_script_items_set_role_is_not_sqlplus_set_tool_command() {
    let sql = "SET ROLE app_role;
SELECT 2 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec![
            "SET ROLE app_role".to_string(),
            "SELECT 2 FROM dual".to_string()
        ],
        "SET ROLE should be parsed as SQL statement, not SQL*Plus SET tool command: {items:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !matches!(item, ScriptItem::ToolCommand(_))),
        "SET ROLE must not produce tool command items: {items:?}"
    );
}

#[test]
fn test_split_script_items_set_constraints_is_not_sqlplus_set_tool_command() {
    let sql = "SET CONSTRAINTS ALL DEFERRED;
SELECT 2 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts,
        vec![
            "SET CONSTRAINTS ALL DEFERRED".to_string(),
            "SELECT 2 FROM dual".to_string()
        ],
        "SET CONSTRAINTS should be parsed as SQL statement, not SQL*Plus SET tool command: {items:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !matches!(item, ScriptItem::ToolCommand(_))),
        "SET CONSTRAINTS must not produce tool command items: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_recursive_search_cycle_set_clauses_not_tool_commands() {
    let sql = "WITH t (id, parent_id) AS (\n  SELECT 1, NULL FROM dual\n  UNION ALL\n  SELECT id + 1, id FROM t WHERE id < 3\n)\nSEARCH DEPTH FIRST BY id SET order_col\nCYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'\nSELECT id, parent_id FROM t;\nSELECT 2 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "recursive WITH SEARCH/CYCLE clauses must remain in a single SQL statement: {stmts:?}"
    );
    assert!(
        stmts[0].contains("SEARCH DEPTH FIRST BY id SET order_col"),
        "SEARCH ... SET clause should remain in first statement: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("CYCLE id SET cycle_mark TO 'Y' DEFAULT 'N'"),
        "CYCLE ... SET clause should remain in first statement: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 2 FROM dual");

    assert!(
        items
            .iter()
            .all(|item| !matches!(item, ScriptItem::ToolCommand(_))),
        "SEARCH/CYCLE SET clauses must not be parsed as SQL*Plus SET tool commands: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_create_view_as_with_function_keeps_single_statement() {
    let sql = "CREATE OR REPLACE VIEW v_with_fn AS\nWITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nSELECT f() AS v FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE VIEW ... AS WITH FUNCTION must remain one statement until main SELECT terminator: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE VIEW v_with_fn AS"),
        "first statement should preserve CREATE VIEW header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("FUNCTION f RETURN NUMBER IS"),
        "first statement should keep WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f() AS v FROM dual"),
        "first statement should include main SELECT body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_create_view_as_with_function_keeps_single_statement() {
    let sql = "CREATE OR REPLACE VIEW v_with_fn AS\nWITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nSELECT f() AS v FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items must keep CREATE VIEW ... AS WITH FUNCTION together: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE VIEW v_with_fn AS"),
        "first formatted statement should preserve CREATE VIEW header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("FUNCTION f RETURN NUMBER IS"),
        "first formatted statement should keep WITH FUNCTION declaration: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("SELECT f() AS v FROM dual"),
        "first formatted statement should include main SELECT body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_with_compound_identifier_splits_normally() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_compound_name
BEFORE INSERT ON t
FOR EACH ROW
DECLARE
  v_compound NUMBER := 1;
BEGIN
  IF v_compound = 1 THEN
    NULL;
  END IF;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger that mentions COMPOUND-like identifier must split after END; got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TRIGGER trg_compound_name"),
        "first statement should preserve trigger body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_when_clause_compound_identifier_splits_normally() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_compound_when
BEFORE INSERT ON t
FOR EACH ROW
WHEN (NEW.COMPOUND IS NULL)
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger WHEN clause identifier COMPOUND must not be parsed as COMPOUND TRIGGER: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE TRIGGER trg_compound_when"),
        "first statement should preserve simple trigger body: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_referencing_new_old_aliases() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_ref_alias
BEFORE INSERT OR UPDATE ON t
REFERENCING NEW AS n OLD AS o
FOR EACH ROW
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger REFERENCING ... AS aliases must not create fake AS/IS block depth: {stmts:?}"
    );
    assert!(
        stmts[0].contains("REFERENCING NEW AS n OLD AS o"),
        "first statement should preserve REFERENCING aliases: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_referencing_new_old_is_aliases() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_ref_alias_is
BEFORE INSERT OR UPDATE ON t
REFERENCING NEW IS n OLD IS o
FOR EACH ROW
IS
BEGIN
  NULL;
END;
SELECT 5 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger REFERENCING ... IS aliases must not consume the body IS header: {stmts:?}"
    );
    assert!(
        stmts[0].contains("REFERENCING NEW IS n OLD IS o"),
        "first statement should preserve REFERENCING IS aliases: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains(
            "FOR EACH ROW
IS
BEGIN"
        ),
        "first statement should preserve trigger body IS header: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 5 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_is_header_splits_normally() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_is_header
BEFORE INSERT ON t
FOR EACH ROW
IS
BEGIN
  NULL;
END;
SELECT 4 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger IS header must keep trigger body as one statement; got: {stmts:?}"
    );
    assert!(
        stmts[0].contains("FOR EACH ROW\nIS\nBEGIN"),
        "first statement should preserve simple trigger IS header: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 4 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_as_header_with_declaration_keeps_single_trigger_statement(
) {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_as_header_decl
BEFORE INSERT ON t
FOR EACH ROW
AS
  v_count NUMBER;
BEGIN
  v_count := 1;
END;
SELECT 6 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger AS header with declaration must not split at declaration semicolon: {stmts:?}"
    );
    assert!(
        stmts[0].contains("AS\n  v_count NUMBER;\nBEGIN"),
        "first statement should preserve AS declarative section: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 6 FROM dual"));
}

#[test]
fn test_split_script_items_simple_trigger_when_with_parenthesized_as_expression() {
    let sql = r#"CREATE OR REPLACE TRIGGER trg_when_case
BEFORE INSERT ON t
FOR EACH ROW
WHEN ((CASE WHEN NEW.status = 'A' THEN 1 ELSE 0 END) = 1)
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "simple trigger WHEN expression containing nested parentheses must not affect AS/IS block detection: {stmts:?}"
    );
    assert!(
        stmts[0].contains("WHEN ((CASE WHEN NEW.status = 'A' THEN 1 ELSE 0 END) = 1)"),
        "first statement should preserve WHEN expression: {}",
        stmts[0]
    );
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_whenever_statement_head() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nWHENEVER SQLERROR EXIT SQL.SQLCODE\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("WHENEVER SQLERROR")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::WheneverSqlError { exit, action }) if *exit && action.as_deref() == Some("SQL.SQLCODE")),
        "second item should parse WHENEVER SQLERROR EXIT SQL.SQLCODE: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_variable_statement_head() {
    let sql = "WITH\n  FUNCTION f RETURN NUMBER IS\n  BEGIN\n    RETURN 1;\n  END;\nVARIABLE v NUMBER\nPRINT v\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("VARIABLE v NUMBER")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Var { name, .. }) if name == "v"),
        "second item should parse VARIABLE command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::ToolCommand(ToolCommand::Print { name }) if name.as_deref() == Some("v")),
        "third item should parse PRINT command: {items:?}"
    );
    assert!(
        matches!(&items[3], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "fourth item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_passw_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
PASSW scott
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("PASSW scott")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "PASSW scott" && message.contains("PASSWORD") && *is_error),
        "second item should classify PASSW command as unsupported SQL*Plus command without leaking into SQL statement: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_connect_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
CONNECT scott/tiger@localhost:1521/ORCL
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("CONNECT scott/tiger@localhost:1521/ORCL")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Connect { username, host, port, service_name, .. }) if username == "scott" && host == "localhost" && *port == 1521 && service_name == "ORCL"),
        "second item should parse CONNECT command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_conn_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
CONN
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("CONN")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { message, is_error: true, .. }) if message.contains("CONNECT requires connection string")),
        "second item should classify bare CONN as CONNECT syntax error command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_tns_connect_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
CONNECT scott/tiger@LOCAL_FREE
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("CONNECT scott/tiger@LOCAL_FREE")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Connect { username, host, port, service_name, .. }) if username == "scott" && host.is_empty() && *port == 0 && service_name == "LOCAL_FREE"),
        "second item should parse TNS CONNECT command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_define_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
DEFINE answer = 42
SELECT &answer FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("DEFINE answer = 42")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Define { name, value }) if name == "answer" && value == "42"),
        "second item should parse DEFINE command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT &answer FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_disconnect_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
DISCONNECT
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("DISCONNECT")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Disconnect)),
        "second item should parse DISCONNECT command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_exit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
EXIT
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("EXIT")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Exit)),
        "second item should parse EXIT command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_quit_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
QUIT
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("QUIT")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Quit)),
        "second item should parse QUIT command: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_password_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
PASSWORD scott
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("PASSWORD scott")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "PASSWORD scott" && message.contains("PASSWORD") && *is_error),
        "second item should classify PASSWORD command as unsupported SQL*Plus command without leaking into SQL statement: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_host_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
HOST ls
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("HOST ls")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "HOST ls" && message.contains("HOST") && *is_error),
        "second item should classify HOST command as unsupported SQL*Plus command without leaking into SQL statement: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_bang_host_statement_head() {
    let sql = "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
! ls
SELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains("! ls")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "! ls" && message.contains("HOST") && *is_error),
        "second item should classify ! host command alias as unsupported SQL*Plus command without leaking into SQL statement: {items:?}"
    );
    assert!(
        matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_sqlplus_report_commands() {
    for report_command in [
        "TIMING START parser_check",
        "TTITLE LEFT 'SPACE Query'",
        "BTITLE LEFT 'Footer'",
        "REPHEADER PAGE",
        "REPFOOTER OFF",
    ] {
        let sql = format!(
            "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
{report_command}
SELECT 2 FROM dual;"
        );
        let items = QueryExecutor::split_script_items(&sql);

        assert!(
            matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains(report_command)),
            "first item should keep only WITH FUNCTION declaration statement: {items:?}"
        );
        assert!(
            matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == report_command && message.contains("report") && *is_error),
            "second item should classify {report_command} as unsupported SQL*Plus report command without leaking into SQL statement: {items:?}"
        );
        assert!(
            matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
            "third item should be trailing SELECT statement: {items:?}"
        );
    }
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_sqlplus_report_commands() {
    for report_command in [
        "TIMING START parser_check",
        "TTITLE LEFT 'SPACE Query'",
        "BTITLE LEFT 'Footer'",
        "REPHEADER PAGE",
        "REPFOOTER OFF",
    ] {
        let sql = format!(
            "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
{report_command}
SELECT 2 FROM dual;"
        );

        let items = QueryExecutor::split_format_items(&sql);
        let stmts: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                FormatItem::Statement(stmt) => Some(stmt.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            stmts.first().is_some_and(|stmt| {
                stmt.contains("WITH")
                    && stmt.contains("FUNCTION f")
                    && !stmt.contains(report_command)
            }),
            "first formatted statement should keep only WITH FUNCTION declaration statement: {stmts:?}"
        );
        assert!(
            matches!(&items[1], FormatItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == report_command && message.contains("report") && *is_error),
            "second item should classify {report_command} as unsupported SQL*Plus report command without leaking into SQL statement: {items:?}"
        );
        assert!(
            stmts
                .get(1)
                .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
            "second formatted statement should be trailing SELECT statement: {stmts:?}"
        );
    }
}

#[test]
fn test_parse_tool_command_classifies_unhandled_sqlplus_set_options_as_unsupported() {
    for set_command in [
        "SET SQLFORMAT CSV",
        "SET APPINFO 'SPACE Query'",
        "SET TERMOUT OFF",
    ] {
        let parsed = QueryExecutor::parse_tool_command(set_command);
        assert!(
            matches!(&parsed, Some(ToolCommand::Unsupported { raw, message, is_error }) if raw == set_command && message.contains("SET command") && *is_error),
            "{set_command} should be classified as unsupported SET command: {parsed:?}"
        );
    }
}

#[test]
fn test_parse_tool_command_accepts_sqlterminator_compatibility_directive() {
    for set_command in ["SET SQLTERMINATOR OFF", "SET SQLTERMINATOR ON;"] {
        let parsed = QueryExecutor::parse_tool_command(set_command);
        assert!(
            matches!(
                &parsed,
                Some(ToolCommand::SqlPlusReportLayout { raw })
                    if raw == set_command.trim_end_matches(';')
            ),
            "{set_command} should be accepted as a validated compatibility directive: {parsed:?}"
        );
    }

    let parsed = QueryExecutor::parse_tool_command("SET SQLTERMINATOR MAYBE");
    assert!(
        matches!(
            &parsed,
            Some(ToolCommand::Unsupported {
                message,
                is_error: true,
                ..
            }) if message.contains("ON or OFF")
        ),
        "invalid SQLTERMINATOR mode must remain an error: {parsed:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_sqlplus_set_options() {
    for set_command in [
        "SET SQLFORMAT CSV",
        "SET APPINFO 'SPACE Query'",
        "SET TERMOUT OFF",
    ] {
        let sql = format!(
            "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
{set_command}
SELECT 2 FROM dual;"
        );
        let items = QueryExecutor::split_script_items(&sql);

        assert!(
            matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains(set_command)),
            "first item should keep only WITH FUNCTION declaration statement: {items:?}"
        );
        assert!(
            matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == set_command && message.contains("SET command") && *is_error),
            "second item should classify {set_command} as unsupported SQL*Plus SET command without leaking into SQL statement: {items:?}"
        );
        assert!(
            matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
            "third item should be trailing SELECT statement: {items:?}"
        );
    }
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_sqlplus_admin_commands() {
    for admin_command in [
        "STARTUP",
        "SHUTDOWN IMMEDIATE",
        "ARCHIVE LOG LIST",
        "RECOVER DATABASE",
    ] {
        let sql = format!(
            "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
{admin_command}
SELECT 2 FROM dual;"
        );
        let items = QueryExecutor::split_script_items(&sql);

        assert!(
            matches!(&items[0], ScriptItem::Statement(stmt) if stmt.contains("FUNCTION f RETURN NUMBER IS") && !stmt.contains(admin_command)),
            "first item should keep only WITH FUNCTION declaration statement: {items:?}"
        );
        assert!(
            matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == admin_command && message.contains("not supported") && *is_error),
            "second item should classify {admin_command} as unsupported SQL*Plus admin command without leaking into SQL statement: {items:?}"
        );
        assert!(
            matches!(&items[2], ScriptItem::Statement(stmt) if stmt.starts_with("SELECT 2 FROM dual")),
            "third item should be trailing SELECT statement: {items:?}"
        );
    }
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_sqlplus_admin_commands() {
    for admin_command in [
        "STARTUP",
        "SHUTDOWN IMMEDIATE",
        "ARCHIVE LOG LIST",
        "RECOVER DATABASE",
    ] {
        let sql = format!(
            "WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
{admin_command}
SELECT 2 FROM dual;"
        );

        let items = QueryExecutor::split_format_items(&sql);
        let stmts: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                FormatItem::Statement(stmt) => Some(stmt.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            stmts.first().is_some_and(|stmt| {
                stmt.contains("WITH") && stmt.contains("FUNCTION f") && !stmt.contains(admin_command)
            }),
            "first formatted statement should keep only WITH FUNCTION declaration statement: {stmts:?}"
        );
        assert!(
            matches!(&items[1], FormatItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == admin_command && message.contains("not supported") && *is_error),
            "second item should classify {admin_command} as unsupported SQL*Plus admin command without leaking into SQL statement: {items:?}"
        );
        assert!(
            stmts
                .get(1)
                .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
            "second formatted statement should be trailing SELECT statement: {stmts:?}"
        );
    }
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_run_script_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
@child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse @child.sql as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_relative_run_script_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
@@child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && *relative_to_caller),
        "second item should parse @@child.sql as relative run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_start_script_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
START child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse START child.sql as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_start_script_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
START child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        stmts.first().is_some_and(|stmt| {
            stmt.contains("WITH")
                && stmt.contains("FUNCTION f")
                && !stmt.contains("START child.sql")
        }),
        "first formatted statement should keep only WITH FUNCTION declaration statement: {stmts:?}"
    );
    assert!(
        matches!(items.get(1), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse START child.sql as run-script command: {items:?}"
    );
    assert!(
        stmts
            .get(1)
            .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "second formatted statement should be trailing SELECT statement: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_run_keyword_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
RUN child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse RUN child.sql as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_run_script_command_strips_inline_comments_from_path() {
    let sql = "@child.sql -- trailing comment
SELECT 1 FROM dual;";

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "first item should parse @ script path without trailing comment: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 1 FROM dual")),
        "second item should keep trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_run_script_variants_strip_inline_comments_from_path() {
    let cases = [
        ("@@child.sql /* keep */ -- trailing", "child.sql", true),
        ("START child.sql /* keep */ -- trailing", "child.sql", false),
        ("RUN child.sql /* keep */ -- trailing", "child.sql", false),
        ("R child.sql -- trailing", "child.sql", false),
    ];

    for (command_line, expected_path, expected_relative) in cases {
        let sql = format!("{command_line}\nSELECT 1 FROM dual;");
        let items = QueryExecutor::split_script_items(sql.as_str());

        assert!(
            matches!(items.first(), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == expected_path && *relative_to_caller == expected_relative),
            "first item should parse script command path without inline comment for {command_line}: {items:?}"
        );
        assert!(
            matches!(items.get(1), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 1 FROM dual")),
            "second item should keep trailing SELECT statement for {command_line}: {items:?}"
        );
    }
}

#[test]
fn test_split_format_items_run_script_command_strips_inline_comments_from_path() {
    let sql = "START child.sql -- trailing comment
SELECT 1 FROM dual;";

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        matches!(items.first(), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "first item should parse START script path without trailing comment: {items:?}"
    );
    assert!(
        stmts
            .first()
            .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 1 FROM dual")),
        "formatted statements should keep trailing SELECT statement: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_r_keyword_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
R child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse R child.sql as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_run_keyword_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
RUN child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        stmts.first().is_some_and(|stmt| {
            stmt.contains("WITH") && stmt.contains("FUNCTION f") && !stmt.contains("RUN child.sql")
        }),
        "first formatted statement should keep only WITH FUNCTION declaration statement: {stmts:?}"
    );
    assert!(
        matches!(items.get(1), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse RUN child.sql as run-script command: {items:?}"
    );
    assert!(
        stmts
            .get(1)
            .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "second formatted statement should be trailing SELECT statement: {stmts:?}"
    );
}

#[test]
fn test_split_format_items_oracle_with_function_recovers_to_r_keyword_statement_head() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
R child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        stmts.first().is_some_and(|stmt| {
            stmt.contains("WITH") && stmt.contains("FUNCTION f") && !stmt.contains("R child.sql")
        }),
        "first formatted statement should keep only WITH FUNCTION declaration statement: {stmts:?}"
    );
    assert!(
        matches!(items.get(1), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse R child.sql as run-script command: {items:?}"
    );
    assert!(
        stmts
            .get(1)
            .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "second formatted statement should be trailing SELECT statement: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_run_buffer_command() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
RUN
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "RUN" && message.contains("without script path") && *is_error),
        "second item should classify RUN without path as unsupported run-buffer command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_oracle_with_function_recovers_to_r_buffer_command() {
    let sql = r#"WITH
  FUNCTION f RETURN NUMBER IS
  BEGIN
    RETURN 1;
  END;
R
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("WITH") && stmt.contains("FUNCTION f")),
        "first item should keep only WITH FUNCTION declaration statement: {items:?}"
    );
    assert!(
        matches!(&items[1], ScriptItem::ToolCommand(ToolCommand::Unsupported { raw, message, is_error }) if raw == "R" && message.contains("without script path") && *is_error),
        "second item should classify R without path as unsupported run-buffer alias: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_script_items_external_language_clause_splits_before_run_script_command() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_cmd RETURN NUMBER
AS LANGUAGE C;
@child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_script_items(sql);

    assert!(
        matches!(items.first(), Some(ScriptItem::Statement(stmt)) if stmt.contains("AS LANGUAGE C") && !stmt.contains("@child.sql")),
        "first item should keep only LANGUAGE call spec statement: {items:?}"
    );
    assert!(
        matches!(items.get(1), Some(ScriptItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse @child.sql as run-script command: {items:?}"
    );
    assert!(
        matches!(items.get(2), Some(ScriptItem::Statement(stmt)) if stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "third item should be trailing SELECT statement: {items:?}"
    );
}

#[test]
fn test_split_format_items_external_language_clause_splits_before_run_script_command() {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_cmd RETURN NUMBER
AS LANGUAGE C;
@child.sql
SELECT 2 FROM dual;"#;

    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        stmts
            .first()
            .is_some_and(|stmt| stmt.contains("AS LANGUAGE C") && !stmt.contains("@child.sql")),
        "first formatted statement should keep only LANGUAGE call spec statement: {stmts:?}"
    );
    assert!(
        matches!(items.get(1), Some(FormatItem::ToolCommand(ToolCommand::RunScript { path, relative_to_caller })) if path == "child.sql" && !relative_to_caller),
        "second item should parse @child.sql as run-script command: {items:?}"
    );
    assert!(
        stmts
            .get(1)
            .is_some_and(|stmt| stmt.trim_start().starts_with("SELECT 2 FROM dual")),
        "second formatted statement should be trailing SELECT statement: {stmts:?}"
    );
}

#[test]
fn test_split_script_items_create_noforce_trigger_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE NOFORCE TRIGGER trg_noforce
BEFORE INSERT ON t
FOR EACH ROW
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE NOFORCE TRIGGER should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE NOFORCE TRIGGER trg_noforce"),
        "first statement should preserve NOFORCE TRIGGER DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_forward_crossedition_trigger_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE FORWARD CROSSEDITION TRIGGER trg_forward
BEFORE INSERT ON t
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE FORWARD CROSSEDITION TRIGGER should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE FORWARD CROSSEDITION TRIGGER trg_forward"),
        "first statement should preserve FORWARD CROSSEDITION TRIGGER DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_reverse_crossedition_trigger_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE REVERSE CROSSEDITION TRIGGER trg_reverse
BEFORE INSERT ON t
BEGIN
  NULL;
END;
SELECT 3 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE REVERSE CROSSEDITION TRIGGER should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE REVERSE CROSSEDITION TRIGGER trg_reverse"),
        "first statement should preserve REVERSE CROSSEDITION TRIGGER DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 3 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_if_not_exists_procedure_splits_before_trailing_select() {
    let sql = r#"CREATE IF NOT EXISTS PROCEDURE p_if_not_exists
IS
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE IF NOT EXISTS PROCEDURE should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE IF NOT EXISTS PROCEDURE p_if_not_exists"),
        "first statement should preserve IF NOT EXISTS PROCEDURE DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_sharing_metadata_procedure_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE SHARING=METADATA PROCEDURE p_sharing
IS
BEGIN
  NULL;
END;
SELECT 2 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE SHARING=METADATA PROCEDURE should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE SHARING=METADATA PROCEDURE p_sharing"),
        "first statement should preserve SHARING=METADATA PROCEDURE DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_sharing_none_function_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE SHARING=NONE FUNCTION f_sharing
RETURN NUMBER
IS
BEGIN
  RETURN 1;
END;
SELECT 3 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE SHARING=NONE FUNCTION should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE SHARING=NONE FUNCTION f_sharing"),
        "first statement should preserve SHARING=NONE FUNCTION DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 3 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_create_sharing_data_package_splits_before_trailing_select() {
    let sql = r#"CREATE OR REPLACE SHARING=DATA PACKAGE pkg_sharing_data AS
  PROCEDURE p;
END pkg_sharing_data;
SELECT 4 FROM dual;"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE SHARING=DATA PACKAGE should split before trailing SELECT, got: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE OR REPLACE SHARING=DATA PACKAGE pkg_sharing_data AS"),
        "first statement should preserve SHARING=DATA PACKAGE DDL: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 4 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_oracle_with_function_without_semicolon_uses_slash_terminator() {
    let sql = "WITH FUNCTION f RETURN NUMBER IS\nBEGIN\n  RETURN 1;\nEND\n/\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "expected WITH FUNCTION and SELECT split, got: {stmts:?}"
    );
    assert!(stmts[0].starts_with("WITH FUNCTION f RETURN NUMBER IS"));
    assert!(stmts[0].contains("RETURN 1;"));
    assert!(stmts[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn test_split_script_items_oracle_with_function_keeps_slash_line_comment_with_main_query() {
    let sql = "WITH FUNCTION f RETURN NUMBER IS\nBEGIN\n  RETURN 1;\nEND\n/ -- sqlplus terminator\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        1,
        "expected WITH FUNCTION main query to keep slash-comment line attached, got: {stmts:?}"
    );
    assert!(stmts[0].starts_with("WITH FUNCTION f RETURN NUMBER IS"));
    assert!(stmts[0].contains("/ -- sqlplus terminator\nSELECT 2 FROM dual"));
}

#[test]
fn test_split_format_items_oracle_with_function_consumes_slash_block_comment_terminator() {
    let sql = "WITH FUNCTION f RETURN NUMBER IS\nBEGIN\n  RETURN 1;\nEND\n/ /* sqlplus terminator */\nSELECT 3 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();
    let slash_count = items
        .iter()
        .filter(|item| matches!(item, FormatItem::Slash))
        .count();

    assert_eq!(
        stmts.len(),
        2,
        "expected WITH FUNCTION and trailing SELECT split by slash block-comment terminator, got: {stmts:?}"
    );
    assert!(stmts[0].starts_with("WITH FUNCTION f RETURN NUMBER IS"));
    assert_eq!(
        slash_count, 1,
        "slash+block-comment line should be consumed as slash format item, got: {items:?}"
    );
    assert!(stmts[1].starts_with("SELECT 3 FROM dual"));
}

#[test]
fn test_split_script_items_external_language_clause_splits_before_parenthesized_query_statement_head(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_paren RETURN NUMBER
AS LANGUAGE C;
(SELECT 2 FROM dual);"#;
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "expected LANGUAGE call spec and parenthesized query split, got: {stmts:?}"
    );
    assert!(stmts[0].contains("AS LANGUAGE C"));
    assert!(stmts[1].starts_with("(SELECT 2 FROM dual)"));
}

#[test]
fn test_statement_bounds_at_cursor_external_language_clause_splits_before_parenthesized_query_statement_head(
) {
    let sql = r#"CREATE OR REPLACE FUNCTION ext_lang_paren RETURN NUMBER
AS LANGUAGE C;
(SELECT 2 FROM dual);"#;
    let cursor = sql.rfind("SELECT 2").unwrap_or(sql.len());

    let bounds = QueryExecutor::statement_bounds_at_cursor(sql, cursor)
        .expect("expected statement bounds for parenthesized trailing query");
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.trim_start().starts_with("(SELECT 2 FROM dual)"),
        "cursor on trailing parenthesized query should resolve only that statement: {statement}"
    );
}

#[test]
fn test_split_script_items_create_sharing_wrapped_procedure_keeps_body_until_slash() {
    let sql = "CREATE OR REPLACE SHARING=METADATA PROCEDURE wrapped_sharing\nWRAPPED\na b c;\nd e f;\n/\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "CREATE SHARING WRAPPED should keep wrapped body semicolons inside one statement until slash delimiter: {stmts:?}"
    );
    assert!(
        stmts[0]
            .starts_with("CREATE OR REPLACE SHARING=METADATA PROCEDURE wrapped_sharing\nWRAPPED"),
        "first statement should preserve SHARING WRAPPED header: {}",
        stmts[0]
    );
    assert!(
        stmts[0].contains("a b c;\nd e f;"),
        "first statement should preserve wrapped body with internal semicolons: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should be trailing SELECT: {}",
        stmts[1]
    );
}

#[test]
fn test_split_script_items_non_ascii_q_quote_delimiter_splits_trailing_statement() {
    let sql = "SELECT q'가x가' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "non-ASCII q-quote delimiter should keep q-quote balanced and split trailing statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("SELECT q'가x가' FROM dual"),
        "first statement should preserve non-ASCII q-quote text: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second statement should remain split: {}",
        stmts[1]
    );
}

#[test]
fn test_split_format_items_non_ascii_q_quote_delimiter_splits_trailing_statement() {
    let sql = "SELECT q'가x가' FROM dual;\nSELECT 2 FROM dual;";
    let items = QueryExecutor::split_format_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            FormatItem::Statement(stmt) => Some(stmt.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        stmts.len(),
        2,
        "split_format_items should keep non-ASCII q-quote delimiter balanced and split trailing statement: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("SELECT q'가x가' FROM dual"),
        "first formatted statement should preserve non-ASCII q-quote text: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("SELECT 2 FROM dual"),
        "second formatted statement should remain split: {}",
        stmts[1]
    );
}

// ── END / END IF / END name nested depth regression tests ──────────────────

#[test]
fn test_line_block_depths_nested_begin_inside_if() {
    // BEGIN inside IF inside outer BEGIN
    let sql = r#"BEGIN
  IF x THEN
    BEGIN
      NULL;
    END;
  END IF;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) IF(1) inner-BEGIN(2) NULL(3) END;(2) END-IF(1) END(0)
    let expected = vec![0, 1, 2, 3, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "BEGIN inside IF depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_loop_with_nested_if() {
    // IF nested inside FOR...LOOP
    let sql = r#"BEGIN
  FOR i IN 1..5 LOOP
    IF x THEN
      NULL;
    END IF;
  END LOOP;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) FOR-LOOP(1) IF(2) NULL(3) END-IF(2) END-LOOP(1) END(0)
    let expected = vec![0, 1, 2, 3, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "IF inside FOR LOOP depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_three_level_nested_if() {
    // Triple-nested IF blocks
    let sql = r#"BEGIN
  IF a THEN
    IF b THEN
      IF c THEN
        NULL;
      END IF;
    END IF;
  END IF;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) IFa(1) IFb(2) IFc(3) NULL(4) END-IFc(3) END-IFb(2) END-IFa(1) END(0)
    let expected = vec![0, 1, 2, 3, 4, 3, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "three-level nested IF depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_standalone_procedure_end_name() {
    // CREATE PROCEDURE with END name (not a package body).
    // BEGIN after IS is shown at the same depth as IS (pre-dedented by 1).
    let sql = r#"CREATE OR REPLACE PROCEDURE my_proc IS
BEGIN
  IF x THEN
    NULL;
  END IF;
END my_proc;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // CREATE IS(0) BEGIN(0) IF(1) NULL(2) END-IF(1) END name(0)
    // Note: BEGIN after IS is at the same depth as IS due to pending_subprogram_begins pre-dedent.
    let expected = vec![0, 0, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "standalone procedure END name depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_split_end_if_inside_package_procedure() {
    // Package body procedure with split END / IF inside.
    // BEGIN after IS is at the same depth as IS (pending_subprogram_begins pre-dedent).
    // Package initializer BEGIN aligns with package scope; its body is one level deeper.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  PROCEDURE p1 IS
  BEGIN
    IF x THEN
      NULL;
    END
    IF;
  END p1;
BEGIN
  NULL;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) PROC IS(1) BEGIN(1) IF(2) NULL(3) END(2) IF;(2) END p1(1) BEGIN(0) NULL(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 3, 2, 2, 1, 0, 1, 0];
    assert_eq!(
        depths, expected,
        "split END IF in package body procedure mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_if_elsif_else_nesting() {
    // IF/ELSIF/ELSE chain inside BEGIN
    let sql = r#"BEGIN
  IF a THEN
    NULL;
  ELSIF b THEN
    NULL;
  ELSE
    NULL;
  END IF;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) IF(1) NULL(2) ELSIF(1) NULL(2) ELSE(1) NULL(2) END-IF(1) END(0)
    let expected = vec![0, 1, 2, 1, 2, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "IF/ELSIF/ELSE depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_multiple_procs_end_names() {
    // Package body with two procedures, each with END name.
    // BEGIN after IS is at the same depth as IS.
    // Package initializer BEGIN aligns with package scope; its body is one level deeper.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END p1;
  PROCEDURE p2 IS
  BEGIN
    NULL;
  END p2;
BEGIN
  NULL;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) p1 IS(1) BEGIN(1) NULL(2) END p1(1) p2 IS(1) BEGIN(1) NULL(2) END p2(1) BEGIN(0) NULL(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 1, 1, 1, 2, 1, 0, 1, 0];
    assert_eq!(
        depths, expected,
        "package body multiple procedures END name depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_end_name_split_across_lines_followed_by_end() {
    // Split END / name in package body followed by package-level END.
    // BEGIN after IS is at the same depth as IS.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  PROCEDURE p1 IS
  BEGIN
    NULL;
  END
  p1;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) PROC IS(1) BEGIN(1) NULL(2) END(1) p1;(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "split END name followed by package END depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_loop_inside_if_with_end_name() {
    // LOOP inside IF inside package body procedure with END name.
    // BEGIN after IS is at the same depth as IS.
    // Package initializer BEGIN aligns with package scope; its body is one level deeper.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  PROCEDURE p1 IS
  BEGIN
    IF x THEN
      FOR i IN 1..5 LOOP
        NULL;
      END LOOP;
    END IF;
  END p1;
BEGIN
  NULL;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) PROC IS(1) BEGIN(1) IF(2) FOR-LOOP(3) NULL(4) END-LOOP(3) END-IF(2) END p1(1) BEGIN(0) NULL(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 3, 4, 3, 2, 1, 0, 1, 0];
    assert_eq!(
        depths, expected,
        "LOOP inside IF inside package procedure END name mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_sequential_end_if_and_end_loop() {
    // Sequential END IF and END LOOP at same nesting level
    let sql = r#"BEGIN
  IF a THEN
    NULL;
  END IF;
  FOR i IN 1..3 LOOP
    NULL;
  END LOOP;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) IF(1) NULL(2) END-IF(1) FOR-LOOP(1) NULL(2) END-LOOP(1) END(0)
    let expected = vec![0, 1, 2, 1, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "sequential END IF + END LOOP depth tracking mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_nested_exception_blocks() {
    // Nested block inside exception handler with its own EXCEPTION section.
    // Outer handler indentation must remain active after the inner END.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_nested_ex AS
  PROCEDURE p_nested IS
  BEGIN
    NULL;
  EXCEPTION
    WHEN OTHERS THEN
      BEGIN
        NULL;
      EXCEPTION
        WHEN OTHERS THEN
          NULL;
      END;
      NULL;
  END p_nested;
END pkg_nested_ex;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    // pkg AS(0) proc IS(1) BEGIN(1) NULL(2) EXCEPTION(1) WHEN(2)
    // inner BEGIN(3) NULL(4) inner EXCEPTION(3) inner WHEN(3) NULL(4) inner END(2)
    // outer handler NULL(3) END p_nested(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 1, 2, 3, 4, 3, 3, 4, 2, 3, 1, 0];
    assert_eq!(
        depths, expected,
        "package body nested exception blocks depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_nested_labeled_declare_does_not_close_parent_routine() {
    let sql = r#"CREATE OR REPLACE PACKAGE BODY fmt_pkg_extreme AS
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
    NULL;
  END validate_and_process;

  PROCEDURE run_extreme (p_root_id IN NUMBER DEFAULT 1, p_text OUT CLOB) IS
    l_modes t_vc_aat;
    l_snapshot CLOB;
  BEGIN
    NULL;
  END run_extreme;
END fmt_pkg_extreme;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let lines: Vec<&str> = sql.lines().collect();

    let apply_one_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PROCEDURE apply_one"))
        .expect("expected PROCEDURE apply_one line");
    let end_inner_rules_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("END inner_rules"))
        .expect("expected END inner_rules line");
    let case_idx = lines
        .iter()
        .position(|line| line.trim_start() == "CASE")
        .expect("expected CASE line");
    let exception_idx = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start() == "EXCEPTION")
        .nth(1)
        .map(|(idx, _)| idx)
        .expect("expected apply_one EXCEPTION line");
    let validate_idx = lines
        .iter()
        .position(|line| {
            line.trim_start()
                .starts_with("PROCEDURE validate_and_process")
        })
        .expect("expected validate_and_process header line");
    let run_extreme_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("PROCEDURE run_extreme"))
        .expect("expected run_extreme header line");
    let run_extreme_decl_idx = lines
        .iter()
        .position(|line| line.trim_start() == "l_snapshot CLOB;")
        .expect("expected run_extreme declaration line");

    assert_eq!(
        depths[end_inner_rules_idx], depths[case_idx],
        "CASE after labeled inner DECLARE block should stay in the same parent routine depth: {depths:?}"
    );
    assert_eq!(
        depths[exception_idx], depths[apply_one_idx],
        "nested routine EXCEPTION should align with its PROCEDURE header depth: {depths:?}"
    );
    assert_eq!(
        depths[validate_idx], depths[run_extreme_idx],
        "package body member headers should keep the same depth after previous nested blocks: {depths:?}"
    );
    assert_eq!(
        depths[run_extreme_decl_idx],
        depths[run_extreme_idx].saturating_add(1),
        "next package body member declarations should remain nested under the recovered member header: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_standalone_nested_exception_blocks() {
    // Same nested exception pattern in standalone routine to cover non-package path.
    let sql = r#"CREATE OR REPLACE PROCEDURE p_nested_ex IS
BEGIN
  NULL;
EXCEPTION
  WHEN OTHERS THEN
    BEGIN
      NULL;
    EXCEPTION
      WHEN OTHERS THEN
        NULL;
    END;
    NULL;
END p_nested_ex;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    // create IS(0) BEGIN(0) NULL(1) EXCEPTION(0) WHEN(1)
    // inner BEGIN(2) NULL(3) inner EXCEPTION(2) inner WHEN(2) NULL(3) inner END(1)
    // outer handler NULL(1) END name(1)
    let expected = vec![0, 0, 1, 0, 1, 2, 3, 2, 2, 3, 1, 1, 1];
    assert_eq!(
        depths, expected,
        "standalone nested exception blocks depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_anonymous_block_with_local_procedure_and_nested_exception_blocks() {
    let sql = r#"DECLARE
  v NUMBER := 0;
  PROCEDURE bump(p IN OUT NUMBER) IS
  BEGIN
    p := p + 1;
  END;
BEGIN
  <<blk1>>
  DECLARE
    a NUMBER := 0;
  BEGIN
    FOR i IN 1..3 LOOP
      bump(a);

      <<blk2>>
      DECLARE
        b NUMBER := 0;
      BEGIN
        WHILE b < 3 LOOP
          b := b + 1;

          BEGIN
            IF (i = 2 AND b = 2) THEN
              RAISE_APPLICATION_ERROR(-20002, 'forced nested error i=2 b=2');
            END IF;

            CASE
              WHEN MOD(i+b,2)=0 THEN
                v := v + 10;
              ELSE
                v := v + 1;
            END CASE;

          EXCEPTION
            WHEN OTHERS THEN
              DBMS_OUTPUT.PUT_LINE('[ANON] caught='||SQLERRM||' -> re-raise once');
              IF i = 2 AND b = 2 THEN
                RAISE;
              END IF;
          END;

        END LOOP;
      END blk2;

    END LOOP;
  END blk1;

EXCEPTION
  WHEN OTHERS THEN
    DBMS_OUTPUT.PUT_LINE('[ANON] top exception handled: '||SQLERRM);
END;
/"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![
        0, 1, 1, 1, 2, 1, 0, 1, 1, 2, 1, 2, 3, 3, 3, 3, 4, 3, 4, 5, 5, 5, 6, 7, 6, 6, 6, 7, 8, 7,
        8, 6, 6, 5, 6, 7, 7, 8, 7, 5, 5, 4, 3, 3, 2, 1, 1, 0, 1, 2, 0, 0,
    ];
    assert_eq!(
        depths, expected,
        "anonymous block with local procedure should keep nested BEGIN/EXCEPTION depths balanced: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_exception_with_split_end_name() {
    // Package body function with EXCEPTION handler and split END / name on next line.
    // Verifies that exception_depth_stack is correctly popped on the split END line,
    // so the label line does not inherit exception handler indentation.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  FUNCTION f1 RETURN NUMBER IS
  BEGIN
    NULL;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN -1;
  END
  f1;
BEGIN
  NULL;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) FN IS(1) BEGIN(1) NULL(2) EXCEPTION(1) WHEN(2) RETURN(3) END(1) f1;(1) BEGIN(0) NULL(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 1, 2, 3, 1, 1, 0, 1, 0];
    assert_eq!(
        depths, expected,
        "package body function with exception handler + split END name depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_triple_nested_end_if_with_split() {
    // Three-level nested IF where the innermost END IF is split across lines.
    let sql = r#"BEGIN
  IF a THEN
    IF b THEN
      IF c THEN
        NULL;
      END
      IF;
    END IF;
  END IF;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) IFa(1) IFb(2) IFc(3) NULL(4) END(3) IF;(3) END-IFb(2) END-IFa(1) END(0)
    let expected = vec![0, 1, 2, 3, 4, 3, 3, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "triple nested IF with split innermost END IF depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_end_if_then_end_loop_then_end() {
    // Mixed END IF / END LOOP / plain END in sequence at different nesting levels
    let sql = r#"BEGIN
  FOR i IN 1..5 LOOP
    IF x THEN
      BEGIN
        NULL;
      END;
    END IF;
  END LOOP;
END;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // BEGIN(0) FOR-LOOP(1) IF(2) BEGIN(3) NULL(4) END;(3) END-IF(2) END-LOOP(1) END(0)
    let expected = vec![0, 1, 2, 3, 4, 3, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "END;/END IF/END LOOP nested sequence depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_standalone_procedure_with_exception() {
    // Standalone procedure with IF and EXCEPTION handler, END name on same line.
    // Verifies exception handler tracking is cleared by END proc_name.
    let sql = r#"CREATE OR REPLACE PROCEDURE test_proc IS
BEGIN
  IF x = 1 THEN
    NULL;
  END IF;
EXCEPTION
  WHEN OTHERS THEN
    NULL;
END test_proc;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // CREATE IS(0) BEGIN(0) IF(1) NULL(2) END-IF(1) EXCEPTION(0) WHEN(1) NULL(2) END name(0)
    let expected = vec![0, 0, 1, 2, 1, 0, 1, 2, 0];
    assert_eq!(
        depths, expected,
        "standalone procedure with exception handler END name depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_nested_if_and_end_names_sequential() {
    // Package body with two procedures: one with nested IF, one plain.
    // Verifies depth resets correctly between procedures.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg AS
  PROCEDURE p1 IS
  BEGIN
    IF x THEN
      IF y THEN
        NULL;
      END IF;
    END IF;
  END p1;
  PROCEDURE p2 IS
  BEGIN
    NULL;
  END p2;
END pkg;"#;
    let depths = QueryExecutor::line_block_depths(sql);
    // AS(0) p1 IS(1) BEGIN(1) IFx(2) IFy(3) NULL(4) END-IFy(3) END-IFx(2) END p1(1)
    // p2 IS(1) BEGIN(1) NULL(2) END p2(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 3, 4, 3, 2, 1, 1, 1, 2, 1, 0];
    assert_eq!(
        depths, expected,
        "package body with nested IF in p1 + plain p2 depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_as_procedure_with_exception_and_end_name() {
    // Ensure PROCEDURE ... AS inside package body follows the same depth behavior as IS.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_as AS
  PROCEDURE p_as AS
  BEGIN
    IF x = 1 THEN
      NULL;
    ELSE
      NULL;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END p_as;
END pkg_as;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    // pkg AS(0) proc AS(1) BEGIN(1) IF(2) NULL(3) ELSE(2) NULL(3) END IF(2)
    // EXCEPTION(1) WHEN(2) NULL(3) END p_as(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 3, 2, 3, 2, 1, 2, 3, 1, 0];
    assert_eq!(
        depths, expected,
        "package body PROCEDURE ... AS with exception/end-name depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_initializer_split_named_end_with_nested_exception_and_suffix_keywords(
) {
    // Extremely nested package initializer with END IF/LOOP/CASE and split package END label.
    // Regression target: split END + label continuation must keep initializer BEGIN at package scope
    // while nesting only the executable body.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY exception AS
BEGIN
  DECLARE
    v NUMBER := 0;
  BEGIN
    IF v = 0 THEN
      FOR i IN 1..2 LOOP
        CASE i
          WHEN 1 THEN NULL;
          ELSE NULL;
        END CASE;
      END LOOP;
    END IF;
  EXCEPTION
    WHEN OTHERS THEN
      NULL;
  END;
END
exception;
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 0, 1, 2, 1, 2, 3, 4, 5, 5, 4, 3, 2, 1, 2, 3, 1, 0, 0, 0];
    assert_eq!(
        depths, expected,
        "package initializer split END label with nested exception/suffix keywords depth mismatch: {depths:?}"
    );

    let items = QueryExecutor::split_script_items(sql);
    let stmts: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(sql) => Some(sql.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        stmts.len(),
        2,
        "expected package body + select, got: {stmts:?}"
    );
    assert!(stmts[0].contains(
        "END
exception"
    ));
    assert_eq!(stmts[1].trim(), "SELECT 1 FROM dual");
}

#[test]
fn test_line_block_depths_package_body_as_with_split_end_if_and_split_end_name() {
    // Verify split END/IF and split END/name are both tracked correctly with AS + EXCEPTION.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_mix AS
  FUNCTION f_mix RETURN NUMBER AS
  BEGIN
    IF flag = 1 THEN
      RETURN 1;
    END
    IF;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN 0;
  END
  f_mix;
END pkg_mix;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    // pkg AS(0) fn AS(1) BEGIN(1) IF(2) RETURN(3) END(2) IF;(2)
    // EXCEPTION(1) WHEN(2) RETURN(3) END(1) f_mix;(1) END pkg(0)
    let expected = vec![0, 1, 1, 2, 3, 2, 2, 1, 2, 3, 1, 1, 0];
    assert_eq!(
        depths, expected,
        "package body AS function split END IF + split END name depth mismatch: {depths:?}"
    );
}

#[test]
fn test_line_block_depths_package_body_open_for_query_then_named_end_returns_to_owner_depth() {
    let sql = r#"create package body a is
    procedure b (c in varchar2) is
    begin
        if (1 = 1) then
            begin
                insert into d (e)
                values (1);
            end;
        end if;
        open v for
            select 1
            from dual;
    end b;
end;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 2, 3, 4, 4, 3, 2, 2, 2, 2, 1, 0];

    assert_eq!(
        depths, expected,
        "package body OPEN ... FOR should not leak query depth into END b; / END; lines: {depths:?}"
    );
}

#[test]
fn test_package_body_keyword_name_with_nested_exception_and_split_schema_end_label() {
    // Stress case: package name is keyword(IF), nested BEGIN/IF/EXCEPTION,
    // and package init END label is schema-qualified/split across lines.
    let sql = r#"CREATE OR REPLACE PACKAGE BODY if AS
  FUNCTION f_if RETURN NUMBER AS
  BEGIN
    BEGIN
      IF 1 = 1 THEN
        RETURN 1;
      END
      IF;
    EXCEPTION
      WHEN OTHERS THEN
        RETURN -1;
    END;
  EXCEPTION
    WHEN OTHERS THEN
      RETURN 0;
  END
  f_if;
BEGIN
  NULL;
EXCEPTION
  WHEN OTHERS THEN
    NULL;
END
owner.if;
/
SELECT 1 FROM dual;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    assert_eq!(
        depths.len(),
        sql.lines().count(),
        "line depth count mismatch"
    );

    let lines: Vec<&str> = sql.lines().collect();
    let end_line = lines
        .iter()
        .rposition(|line| line.trim() == "END")
        .unwrap_or(0);
    let label_line = lines
        .iter()
        .position(|line| line.trim() == "owner.if;")
        .unwrap_or(0);
    let select_line = lines
        .iter()
        .position(|line| line.trim() == "SELECT 1 FROM dual;")
        .unwrap_or(0);

    assert_eq!(
        depths[end_line], depths[label_line],
        "split package END / label should keep same depth: {depths:?}"
    );
    assert_eq!(
        depths[select_line], 0,
        "depth should be reset before trailing SELECT: {depths:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// MySQL/MariaDB – additional edge-case tests
// ──────────────────────────────────────────────────────────────────────────────

/// `--` must be usable as a DELIMITER value.
/// Previously `parse_mysql_delimiter_directive` stopped at `--` thinking it
/// was a comment, returning the default `;` instead.
#[test]
fn test_parse_mysql_delimiter_directive_double_dash_as_delimiter_value() {
    use crate::sql_text::parse_mysql_delimiter_directive;
    assert_eq!(
        parse_mysql_delimiter_directive("DELIMITER --"),
        Some("--".to_string()),
        "-- must be accepted as a literal delimiter value"
    );
}

/// `#` must be usable as a DELIMITER value.
/// Previously the function stopped at `#` thinking it was a MySQL hash comment.
#[test]
fn test_parse_mysql_delimiter_directive_hash_as_delimiter_value() {
    use crate::sql_text::parse_mysql_delimiter_directive;
    assert_eq!(
        parse_mysql_delimiter_directive("DELIMITER #"),
        Some("#".to_string()),
        "# must be accepted as a literal delimiter value"
    );
}

/// `//` is commonly used as an alternative delimiter.
#[test]
fn test_parse_mysql_delimiter_directive_double_slash_as_delimiter_value() {
    use crate::sql_text::parse_mysql_delimiter_directive;
    assert_eq!(
        parse_mysql_delimiter_directive("DELIMITER //"),
        Some("//".to_string())
    );
}

/// A single-quoted string that contains the custom delimiter characters
/// must NOT cause the statement to be split early.
#[test]
fn test_split_script_items_mysql_delimiter_string_shields_delimiter_chars() {
    let sql = "DELIMITER $$\nSELECT '$$' AS val$$\nDELIMITER ;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        1,
        "delimiter chars inside a string literal must not terminate the statement early: {stmts:?}"
    );
    assert!(
        stmts[0].contains("'$$'"),
        "string literal with delimiter chars should be preserved intact: {}",
        stmts[0]
    );
}

/// A backtick-quoted identifier that contains the custom delimiter characters
/// must NOT cause the statement to be split early.
#[test]
fn test_split_script_items_mysql_delimiter_backtick_shields_delimiter_chars() {
    let sql = "DELIMITER $$\nSELECT `col$$` FROM t$$\nDELIMITER ;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        1,
        "delimiter chars inside a backtick identifier must not terminate the statement early: {stmts:?}"
    );
    assert!(
        stmts[0].contains("`col$$`"),
        "backtick identifier with delimiter chars should be preserved: {}",
        stmts[0]
    );
}

/// Two consecutive stored procedures separated by `$$` must each be emitted
/// as a separate statement.
#[test]
fn test_split_script_items_mysql_multiple_procedures_with_custom_delimiter() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE proc1()
BEGIN
  SELECT 1;
END$$
CREATE PROCEDURE proc2()
BEGIN
  SELECT 2;
END$$
DELIMITER ;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "two $$-terminated procedures should produce two statements: {stmts:?}"
    );
    assert!(
        stmts[0].contains("proc1") && stmts[0].contains("SELECT 1"),
        "first statement should be proc1: {}",
        stmts[0]
    );
    assert!(
        stmts[1].contains("proc2") && stmts[1].contains("SELECT 2"),
        "second statement should be proc2: {}",
        stmts[1]
    );
}

/// A LOOP body must remain a single statement when using a custom delimiter.
#[test]
fn test_split_script_items_mysql_loop_body_with_custom_delimiter() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE loop_test()
BEGIN
  DECLARE i INT DEFAULT 0;
  my_loop: LOOP
    SET i = i + 1;
    IF i >= 3 THEN
      LEAVE my_loop;
    END IF;
  END LOOP my_loop;
END$$
DELIMITER ;
SELECT 1;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "LOOP procedure + trailing SELECT should be 2 statements: {stmts:?}"
    );
    assert!(
        stmts[0].contains("LOOP") && stmts[0].contains("LEAVE"),
        "LOOP body must stay intact: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 1");
}

/// A trigger body with an IF statement must remain a single statement
/// when using a custom delimiter.
#[test]
fn test_split_script_items_mysql_trigger_body_with_custom_delimiter() {
    let sql = r#"DELIMITER $$
CREATE TRIGGER before_insert_check
BEFORE INSERT ON orders FOR EACH ROW
BEGIN
  IF NEW.amount <= 0 THEN
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'amount must be positive';
  END IF;
END$$
DELIMITER ;
SELECT 1;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "trigger definition + trailing SELECT should be 2 statements: {stmts:?}"
    );
    assert!(
        stmts[0].contains("TRIGGER") && stmts[0].contains("SIGNAL"),
        "trigger body must stay intact: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 1");
}

#[test]
fn test_split_script_items_mysql_mixed_routines_with_custom_delimiter() {
    let sql = r#"DELIMITER $$
CREATE TRIGGER bi_task_log
    BEFORE INSERT
    ON task_log
    FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL
            OR NEW.hours <= 0
            OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
    IF NEW.payload IS NULL
            OR JSON_VALID (NEW.payload) = 0 THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'payload must be valid JSON';
    END IF;
    IF NEW.note IS NULL
            OR CHAR_LENGTH (TRIM (NEW.note)) = 0 THEN
        SET NEW.note = CONCAT ('auto-note:', NEW.employee_id, ':', DATE_FORMAT (NEW.work_date, '%Y%m%d'));
    END IF;
END$$

CREATE TRIGGER ai_task_log
    AFTER INSERT
    ON task_log
    FOR EACH ROW
BEGIN
    INSERT INTO audit_events (event_type, entity_name, entity_id, detail)
    VALUES ('TASK_INSERT', 'task_log', NEW.log_id, JSON_OBJECT ('project_id', NEW.project_id, 'employee_id', NEW.employee_id, 'hours', NEW.hours, 'work_date', DATE_FORMAT (NEW.work_date, '%Y-%m-%d'), 'note', NEW.note));
END$$

CREATE FUNCTION fn_efficiency_band (
    p_hours DECIMAL (12, 2),
    p_budget DECIMAL (14, 2)
) RETURNS VARCHAR (16) DETERMINISTIC
BEGIN
    DECLARE v_score DECIMAL (12, 4);
    SET v_score = IFNULL (p_hours / NULLIF (p_budget / 1000, 0), 0);
    RETURN
    CASE
        WHEN v_score >= 1.20 THEN
            'critical'
        WHEN v_score >= 0.80 THEN
            'watch'
        ELSE
            'ok'
    END;
END$$

CREATE PROCEDURE sp_assert_eq_bigint (
    IN p_expected BIGINT,
    IN p_actual BIGINT,
    IN p_message VARCHAR (255)
)
BEGIN
    IF NOT (p_expected <=> p_actual) THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = CONCAT ('ASSERT: ', p_message);
    END IF;
END$$"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        4,
        "custom-delimiter script should split trigger/function/procedure definitions independently: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE TRIGGER bi_task_log"),
        "first statement should be the BEFORE INSERT trigger: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("CREATE TRIGGER ai_task_log"),
        "second statement should be the AFTER INSERT trigger: {}",
        stmts[1]
    );
    assert!(
        stmts[2].starts_with("CREATE FUNCTION fn_efficiency_band"),
        "third statement should be the function: {}",
        stmts[2]
    );
    assert!(
        stmts[3].starts_with("CREATE PROCEDURE sp_assert_eq_bigint"),
        "fourth statement should be the procedure: {}",
        stmts[3]
    );
}

#[test]
fn test_split_script_items_mysql_session_delimiter_splits_triggers_without_directive() {
    let sql = r#"CREATE TRIGGER bi_task_log
BEFORE INSERT ON task_log
FOR EACH ROW
BEGIN
    IF NEW.hours IS NULL OR NEW.hours <= 0 OR NEW.hours > 24 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'hours must be > 0 and <= 24';
    END IF;
END$$

CREATE TRIGGER ai_task_log
AFTER INSERT ON task_log
FOR EACH ROW
BEGIN
    INSERT INTO audit_events (
        event_type,
        entity_name,
        entity_id,
        detail
    )
    VALUES (
        'TASK_INSERT',
        'task_log',
        NEW.log_id,
        JSON_OBJECT(
            'project_id', NEW.project_id,
            'employee_id', NEW.employee_id,
            'hours', NEW.hours,
            'work_date', DATE_FORMAT(NEW.work_date, '%Y-%m-%d'),
            'note', NEW.note
        )
    );
END$$"#;

    let items = QueryExecutor::split_script_items_for_db_type_with_mysql_delimiter(
        sql,
        Some(crate::db::connection::DatabaseType::MySQL),
        Some("$$"),
    );
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "session mysql delimiter should split both triggers even when the selection omits the DELIMITER directive: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("CREATE TRIGGER bi_task_log"),
        "first split statement should be the BEFORE trigger: {}",
        stmts[0]
    );
    assert!(
        stmts[1].starts_with("CREATE TRIGGER ai_task_log"),
        "second split statement should be the AFTER trigger: {}",
        stmts[1]
    );
}

/// A double-quoted string containing the custom delimiter must be shielded.
#[test]
fn test_split_script_items_mysql_double_quoted_string_shields_delimiter_chars() {
    let sql = "DELIMITER $$\nSELECT \"val$$\" AS x$$\nDELIMITER ;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        1,
        "delimiter chars inside a double-quoted string must not terminate early: {stmts:?}"
    );
    assert!(
        stmts[0].contains("\"val$$\""),
        "double-quoted string with delimiter chars should be preserved: {}",
        stmts[0]
    );
}

/// WHILE loop inside a procedure must stay as a single statement.
#[test]
fn test_split_script_items_mysql_while_loop_with_custom_delimiter() {
    let sql = r#"DELIMITER $$
CREATE PROCEDURE while_test()
BEGIN
  DECLARE n INT DEFAULT 0;
  WHILE n < 5 DO
    SET n = n + 1;
  END WHILE;
END$$
DELIMITER ;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        1,
        "WHILE procedure should be one statement: {stmts:?}"
    );
    assert!(
        stmts[0].contains("WHILE") && stmts[0].contains("END WHILE"),
        "WHILE body must stay intact: {}",
        stmts[0]
    );
}

#[test]
fn test_line_block_depths_mariadb_begin_not_atomic_declare_does_not_create_extra_block() {
    let sql = r#"BEGIN NOT ATOMIC
  DECLARE v_count INT DEFAULT 0;
  SET v_count = v_count + 1;
END;"#;

    let depths = QueryExecutor::line_block_depths(sql);
    let expected = vec![0, 1, 1, 0];

    assert_eq!(
        depths, expected,
        "MariaDB BEGIN NOT ATOMIC block should keep DECLARE at statement-body depth: {depths:?}"
    );
}

#[test]
fn test_split_script_items_mariadb_begin_not_atomic_with_custom_delimiter_stays_single_statement() {
    let sql = r#"DELIMITER $$
BEGIN NOT ATOMIC
  DECLARE v_count INT DEFAULT 0;
  SET v_count = v_count + 1;
END$$
DELIMITER ;
SELECT 1;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "MariaDB BEGIN NOT ATOMIC block should stay intact before the trailing SELECT: {stmts:?}"
    );
    assert!(
        stmts[0].contains("BEGIN NOT ATOMIC")
            && stmts[0].contains("DECLARE v_count INT DEFAULT 0;")
            && stmts[0].contains("SET v_count = v_count + 1;"),
        "first statement should preserve the full anonymous compound block: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 1");
}

#[test]
fn test_split_script_items_mariadb_begin_not_atomic_with_default_delimiter_stays_single_statement()
{
    let sql = r#"BEGIN NOT ATOMIC
  DECLARE v_count INT DEFAULT 0;
  SET v_count = v_count + 1;
END;
SELECT 1;
"#;

    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);

    assert_eq!(
        stmts.len(),
        2,
        "MariaDB BEGIN NOT ATOMIC block should stay intact even with the default delimiter: {stmts:?}"
    );
    assert!(
        stmts[0].starts_with("BEGIN NOT ATOMIC")
            && stmts[0].contains("DECLARE v_count INT DEFAULT 0;")
            && stmts[0].contains("SET v_count = v_count + 1;")
            && stmts[0].ends_with("END"),
        "first statement should preserve the full anonymous compound block: {}",
        stmts[0]
    );
    assert_eq!(stmts[1], "SELECT 1");
}

#[test]
fn test_statement_bounds_at_cursor_mariadb_begin_not_atomic_auto_detects_mysql_mode() {
    let sql = r#"BEGIN NOT ATOMIC
  DECLARE v_count INT DEFAULT 0;
  SET v_count = v_count + 1;
END;
SELECT 1;
"#;
    let cursor = sql.find("SET v_count").unwrap_or(0);
    let bounds = QueryExecutor::statement_bounds_at_cursor_for_db_type(sql, cursor, None)
        .unwrap_or_else(|| panic!("expected anonymous MariaDB block bounds at cursor {cursor}"));
    let statement = &sql[bounds.0..bounds.1];

    assert!(
        statement.starts_with("BEGIN NOT ATOMIC"),
        "cursor inside anonymous MariaDB block should resolve the block statement, got:\n{statement}"
    );
    assert!(
        statement.contains("DECLARE v_count INT DEFAULT 0;")
            && statement.contains("SET v_count = v_count + 1;")
            && statement.ends_with("END"),
        "anonymous MariaDB block bounds should keep the whole block body, got:\n{statement}"
    );
    assert!(
        !statement.contains("SELECT 1;"),
        "following statement must not leak into anonymous MariaDB block bounds"
    );
}

#[test]
fn transaction_feedback_policy_is_single_sourced_per_database_type() {
    use crate::db::connection::DatabaseType;
    use result_messages::{transaction_feedback_flag, TransactionFeedbackStatement};

    // Oracle: DML reports both states; procedure-like work reports
    // feedback only when client auto-commit resolved it.
    assert_eq!(
        transaction_feedback_flag(
            DatabaseType::Oracle,
            TransactionFeedbackStatement::Dml,
            true
        ),
        Some(true)
    );
    assert_eq!(
        transaction_feedback_flag(
            DatabaseType::Oracle,
            TransactionFeedbackStatement::Dml,
            false
        ),
        Some(false)
    );
    assert_eq!(
        transaction_feedback_flag(
            DatabaseType::Oracle,
            TransactionFeedbackStatement::ProcedureLike,
            true
        ),
        Some(true)
    );
    assert_eq!(
        transaction_feedback_flag(
            DatabaseType::Oracle,
            TransactionFeedbackStatement::ProcedureLike,
            false
        ),
        None
    );

    // MySQL and MariaDB: DML and procedure calls both report either state.
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        for statement in [
            TransactionFeedbackStatement::Dml,
            TransactionFeedbackStatement::ProcedureLike,
        ] {
            assert_eq!(
                transaction_feedback_flag(db_type, statement, true),
                Some(true)
            );
            assert_eq!(
                transaction_feedback_flag(db_type, statement, false),
                Some(false)
            );
        }
    }
}

#[test]
fn apply_transaction_feedback_appends_only_when_policy_selects_the_statement() {
    use crate::db::connection::DatabaseType;
    use result_messages::{apply_transaction_feedback, TransactionFeedbackStatement};

    assert_eq!(
        apply_transaction_feedback(
            "UPDATE 1 row(s) affected",
            DatabaseType::Oracle,
            Some(TransactionFeedbackStatement::Dml),
            false
        ),
        "UPDATE 1 row(s) affected | Commit required"
    );
    assert_eq!(
        apply_transaction_feedback(
            "PL/SQL block executed successfully",
            DatabaseType::Oracle,
            Some(TransactionFeedbackStatement::ProcedureLike),
            false
        ),
        "PL/SQL block executed successfully"
    );
    // DDL and other statements carry no feedback on any backend.
    assert_eq!(
        apply_transaction_feedback("Table created", DatabaseType::Oracle, None, true),
        "Table created"
    );
}

#[test]
fn shared_result_message_formatters_match_executor_output() {
    assert_eq!(
        result_messages::dml_rows_affected("UPDATE", 7),
        "UPDATE 7 row(s) affected"
    );
    assert_eq!(
        result_messages::with_out_binds("Call executed successfully", &[":V = 1".to_string()]),
        "Call executed successfully | OUT: :V = 1"
    );
    assert_eq!(
        result_messages::with_out_binds("Call executed successfully", &[]),
        "Call executed successfully"
    );
    assert_eq!(
        result_messages::script_select_batch_progress("1 rows fetched", 1, 2),
        "1 rows fetched (Executed 1 of 2 statements)"
    );
    assert_eq!(
        result_messages::script_batch_summary(2, 2, 7, &[]),
        "Executed 2 statements, 7 row(s) affected"
    );
    assert_eq!(
        result_messages::script_batch_summary(1, 2, 7, &["Statement 2: ORA-00900".to_string()]),
        "Executed 1 of 2 statements, 7 row(s) affected | Errors: Statement 2: ORA-00900"
    );
}

#[test]
fn test_extract_bind_names_skips_renamed_trigger_correlations() {
    // REFERENCING renames the pseudo-records; the renamed forms are still not binds.
    let sql = r#"CREATE OR REPLACE TRIGGER sq_iov_trg
INSTEAD OF INSERT OR UPDATE ON sq_iov
REFERENCING NEW AS incoming OLD AS existing
FOR EACH ROW
BEGIN
  INSERT INTO sq_fact (fact_id, qty) VALUES (:incoming.fact_id, :incoming.qty);
  UPDATE sq_fact SET qty = :incoming.qty WHERE fact_id = :existing.fact_id;
  :NEW.audited := :audit_user;
END;"#;
    let names = QueryExecutor::extract_bind_names(sql);
    assert_eq!(
        names.len(),
        1,
        "only the real bind should survive, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.to_uppercase() == "AUDIT_USER"),
        "Should contain AUDIT_USER, got: {:?}",
        names
    );
}

#[test]
fn test_trigger_correlation_names_defaults_without_referencing() {
    let sql = r#"CREATE OR REPLACE TRIGGER sq_plain_trg
BEFORE INSERT ON sq_fact
FOR EACH ROW
BEGIN
  :NEW.qty := 1;
END;"#;
    let correlations = QueryExecutor::trigger_correlation_names(sql);
    assert!(correlations.contains("NEW"));
    assert!(correlations.contains("OLD"));
    assert!(correlations.contains("PARENT"));
    assert!(!correlations.contains("QTY"));
}

#[test]
fn order_by_direction_with_inline_comment_does_not_split_the_statement() {
    // `DESC` doubles as SQL*Plus DESCRIBE; a trailing comment must not turn an
    // ORDER BY continuation into a tool command.
    let sql = "SELECT a\nFROM t\nORDER BY b /*m*/\n    DESC /*n*/\n    NULLS FIRST;\n\nSELECT 99 FROM dual;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "unexpected statements: {stmts:?}");
    assert!(stmts[0].contains("NULLS FIRST"), "got: {}", stmts[0]);
}

#[test]
fn describe_tool_command_still_recognised_with_an_object_name() {
    let sql = "SELECT 1 FROM dual;\nDESC sq_hard_metric\n";
    let items = QueryExecutor::split_script_items(sql);
    assert!(
        items.iter().any(|item| matches!(
            item,
            crate::db::ScriptItem::ToolCommand(crate::db::ToolCommand::Describe { name })
                if name.eq_ignore_ascii_case("sq_hard_metric")
        )),
        "DESCRIBE with an object name must stay a tool command: {items:?}"
    );
}

#[test]
fn tool_command_shaped_lines_never_cut_an_open_statement() {
    // Every one of these continuation lines starts with a word that is also a
    // client command (SQL*Plus COLUMN/DESCRIBE, MySQL USE); none of them may end
    // the statement that is still open above it.
    let cases: &[(&str, &str)] = &[
        (
            "alter-add-column",
            "ALTER TABLE t\n  ADD\n  COLUMN c INT;\n\nSELECT 99 FROM dual;\n",
        ),
        (
            "alter-add-constraint",
            "ALTER TABLE t\n  ADD\n  CONSTRAINT ck CHECK (a > 0);\n\nSELECT 99 FROM dual;\n",
        ),
        (
            "mysql-index-hint",
            "SELECT a FROM t\nUSE INDEX (ix)\nWHERE a = 1;\n\nSELECT 99 FROM dual;\n",
        ),
        (
            "order-by-direction-with-separator",
            "SELECT a FROM t\nORDER BY b\n  DESC NULLS LAST,\n  c;\n\nSELECT 99 FROM dual;\n",
        ),
        (
            "order-by-direction-with-comment",
            "SELECT a FROM t\nORDER BY b\n  DESC /*n*/,\n  c;\n\nSELECT 99 FROM dual;\n",
        ),
    ];

    for (name, sql) in cases {
        let items = QueryExecutor::split_script_items(sql);
        let stmts = get_statements(&items);
        assert_eq!(stmts.len(), 2, "{name} split unexpectedly: {stmts:?}");
        assert!(
            stmts[1].contains("SELECT 99"),
            "{name} lost the following statement: {stmts:?}"
        );
    }
}

#[test]
fn case_expression_end_does_not_arm_a_loop_from_a_column_alias() {
    let sql = "SELECT CASE WHEN 1 = 1 THEN 'a' END loop FROM t;\n\nSELECT 99 FROM dual;\n";
    let items = QueryExecutor::split_script_items(sql);
    let stmts = get_statements(&items);
    assert_eq!(stmts.len(), 2, "unexpected statements: {stmts:?}");
    assert!(stmts[0].contains("END loop FROM t"), "got: {}", stmts[0]);
}

#[test]
fn trigger_correlation_names_cover_every_referencing_spelling() {
    // PARENT (nested-table triggers), a bare alias without AS, and the default
    // pseudo-records all have to be recognised as correlations, not binds.
    let sql = r#"CREATE OR REPLACE TRIGGER sq_nested_trg
INSTEAD OF INSERT ON sq_nested_view
REFERENCING NEW AS incoming OLD existing PARENT AS owner_row
FOR EACH ROW
BEGIN
  INSERT INTO sq_fact (a, b, c, d)
  VALUES (:incoming.a, :existing.b, :owner_row.c, :NEW.d);
  :OLD.e := :PARENT.f;
  :audit_user := 1;
END;"#;
    let names = QueryExecutor::extract_bind_names(sql);
    assert_eq!(
        names.len(),
        1,
        "only the real bind should survive: {names:?}"
    );
    assert!(names.iter().any(|name| name.to_uppercase() == "AUDIT_USER"));
}

#[test]
fn mysql_dash_ruler_is_a_comment_not_a_chain_of_minus_operators() {
    for line in ["--------", "--------   ", "-- ", "----\t"] {
        assert!(
            crate::sql_text::is_mysql_dash_comment_start(line.as_bytes(), 0),
            "{line:?} should open a comment"
        );
    }
    for line in ["---x", "-x", "-"] {
        assert!(
            !crate::sql_text::is_mysql_dash_comment_start(line.as_bytes(), 0),
            "{line:?} must not open a comment"
        );
    }
}

/// An MLE module body is JavaScript: its `;`, `//` comment and template literal
/// must not split the statement or open SQL lexer states.
#[test]
fn test_split_script_items_keeps_mle_module_javascript_body_intact() {
    let sql = "CREATE OR REPLACE MLE MODULE js_mod LANGUAGE JAVASCRIPT AS\n\
export function addUp(a, b) {\n\
  const parts = [a, b].map((n) => n * 1);   // SELECT * FROM dual; not SQL\n\
  const banner = `${a} -- /* still text */`;\n\
  return parts.reduce((acc, n) => acc + n, 0);\n\
}\n\
/\n\
SELECT 1 AS after_module FROM dual;\n";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&String> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(statement) => Some(statement),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 2, "unexpected split: {items:?}");
    assert!(
        statements[0].contains("// SELECT * FROM dual; not SQL")
            && statements[0].contains("reduce((acc, n) => acc + n, 0)"),
        "module body was cut: {:?}",
        statements[0]
    );
    assert!(
        statements[1].contains("after_module"),
        "statement after the module body was lost: {:?}",
        statements[1]
    );
}

/// The inline `{{ ... }}` call-spec body is foreign source too, braces and all.
#[test]
fn test_split_script_items_keeps_inline_mle_body_intact() {
    let sql = "CREATE OR REPLACE FUNCTION reverse_text (word VARCHAR2) RETURN VARCHAR2\n\
AS MLE LANGUAGE JAVASCRIPT\n\
{{\n\
  const map = { first: 1, second: 2 };\n\
  return [...WORD].reverse().join('') + map.first;\n\
}};\n\
/\n\
SELECT 2 AS after_inline FROM dual;\n";
    let items = QueryExecutor::split_script_items(sql);
    let statements: Vec<&String> = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(statement) => Some(statement),
            _ => None,
        })
        .collect();

    assert_eq!(statements.len(), 2, "unexpected split: {items:?}");
    assert!(
        statements[0].contains("{ first: 1, second: 2 }")
            && statements[0].contains("reverse().join('')"),
        "inline body was cut: {:?}",
        statements[0]
    );
}

/// `cond ? a : b` inside foreign source is a JavaScript ternary, not a bind.
#[test]
fn test_extract_bind_names_skips_foreign_language_source() {
    let module = "CREATE OR REPLACE MLE MODULE js_mod LANGUAGE JAVASCRIPT AS\n\
export function shout(text) {\n\
  const banner = `${text}!`;\n\
  return banner === banner.toUpperCase() ? banner : banner.toUpperCase();\n\
}\n";
    assert!(
        QueryExecutor::extract_bind_names(module).is_empty(),
        "javascript ternary must not read as a bind variable"
    );

    let inline = "CREATE OR REPLACE FUNCTION pick (flag NUMBER) RETURN VARCHAR2\n\
AS MLE LANGUAGE JAVASCRIPT\n\
{{ return FLAG ? 'yes' : 'no'; }};";
    assert!(
        QueryExecutor::extract_bind_names(inline).is_empty(),
        "inline javascript ternary must not read as a bind variable"
    );

    // A real bind in ordinary SQL is still found.
    assert_eq!(
        QueryExecutor::extract_bind_names("SELECT :wanted FROM dual"),
        vec!["WANTED".to_string()]
    );
}

/// A bodyless stored unit owns its terminator: dropping it stores the unit
/// incomplete and the next call fails to compile.
#[test]
fn test_normalize_sql_for_execute_keeps_stored_unit_call_spec_semicolon() {
    for sql in [
        "CREATE OR REPLACE FUNCTION addup (a NUMBER, b NUMBER) RETURN NUMBER AS\n  MLE MODULE js_mod\n  SIGNATURE 'addUp(number, number)';",
        "CREATE OR REPLACE FUNCTION product (input NUMBER) RETURN NUMBER\n  PARALLEL_ENABLE AGGREGATE USING product_t;",
        "CREATE OR REPLACE FUNCTION widen (src SYS_REFCURSOR) RETURN tab_t PIPELINED\n  ROW POLYMORPHIC USING widen_pkg;",
    ] {
        assert!(
            QueryExecutor::normalize_sql_for_execute(sql).ends_with(';'),
            "call spec lost its mandatory terminator: {sql:?}"
        );
    }

    // Ordinary DDL still has its terminator stripped.
    assert!(
        !QueryExecutor::normalize_sql_for_execute("CREATE TABLE t (id NUMBER);").ends_with(';'),
        "CREATE TABLE must not keep a trailing terminator"
    );
}

/// SQL*Plus `COMPUTE <fn> LABEL <text> OF <col> ON <group>`; the label may be
/// quoted and may contain spaces.
#[test]
fn test_compute_label_command_parsed() {
    let items =
        QueryExecutor::split_script_items("COMPUTE SUM LABEL 'grand total' OF val ON report");
    let compute = items
        .iter()
        .find_map(|item| match item {
            ScriptItem::ToolCommand(command @ ToolCommand::Compute { .. }) => Some(command),
            _ => None,
        })
        .unwrap_or_else(|| panic!("COMPUTE ... LABEL was not recognized: {items:?}"));

    assert!(
        matches!(
            compute,
            ToolCommand::Compute {
                mode: crate::db::ComputeMode::Sum,
                label: Some(label),
                of_column: Some(of_column),
                on_column: Some(on_column),
            } if label == "grand total" && of_column == "val" && on_column == "report"
        ),
        "unexpected COMPUTE parse: {compute:?}"
    );

    let unquoted = QueryExecutor::split_script_items("COMPUTE COUNT LABEL rows OF id ON grp");
    assert!(
        unquoted.iter().any(|item| matches!(
            item,
            ScriptItem::ToolCommand(ToolCommand::Compute { label: Some(label), .. })
                if label == "rows"
        )),
        "unquoted COMPUTE label was not recognized: {unquoted:?}"
    );
}

#[test]
fn test_standalone_slash_repeats_semicolon_terminated_oracle_sql() {
    let items = QueryExecutor::split_script_items_for_db_type(
        "SELECT 1 AS value FROM dual;\n/",
        Some(crate::db::DatabaseType::Oracle),
    );
    let statements = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(statement) => Some(statement.as_str()),
            ScriptItem::ToolCommand(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        statements.len(),
        2,
        "unexpected slash repeat split: {items:?}"
    );
    assert_eq!(statements[0], statements[1]);
}

#[test]
fn test_slash_does_not_repeat_sql_when_sqlterminator_is_off() {
    let items = QueryExecutor::split_script_items_for_db_type(
        "SET SQLTERMINATOR OFF\n\
INSERT ALL INTO demo_table (id) VALUES (1) SELECT 1 FROM dual;\n\
/\n\
SET SQLTERMINATOR ON",
        Some(crate::db::DatabaseType::Oracle),
    );
    let statements = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statements, 1,
        "slash must only terminate while SQLTERMINATOR is off: {items:?}"
    );
}

#[test]
fn test_second_slash_repeats_sql_when_sqlterminator_is_off() {
    let items = QueryExecutor::split_script_items_for_db_type(
        "SET SQLTERMINATOR OFF\n\
INSERT ALL INTO demo_table (id) VALUES (1) SELECT 1 FROM dual;\n\
/\n\
-- A comment-only buffer does not replace the preceding SQL in SQL*Plus.\n\
/\n\
SET SQLTERMINATOR ON",
        Some(crate::db::DatabaseType::Oracle),
    );
    let statements = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statements, 2,
        "second slash must replay the SQL*Plus buffer: {items:?}"
    );
}

#[test]
fn test_slash_terminator_does_not_repeat_open_oracle_block() {
    let items = QueryExecutor::split_script_items_for_db_type(
        "BEGIN\n  NULL;\nEND;\n/",
        Some(crate::db::DatabaseType::Oracle),
    );
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statement_count, 1,
        "block slash must only terminate: {items:?}"
    );
}

#[test]
fn test_standalone_slash_does_not_repeat_oracle_execute_command() {
    let items = QueryExecutor::split_script_items_for_db_type(
        "EXECUTE demo_proc('value')\n/",
        Some(crate::db::DatabaseType::Oracle),
    );
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statement_count, 1,
        "EXECUTE slash must only terminate: {items:?}"
    );
}

#[test]
fn test_slash_does_not_repeat_with_local_oracle_function() {
    let sql = "WITH\n\
FUNCTION f RETURN NUMBER IS\n\
BEGIN\n\
  RETURN 1;\n\
END;\n\
SELECT f AS value FROM dual;\n/";
    let items =
        QueryExecutor::split_script_items_for_db_type(sql, Some(crate::db::DatabaseType::Oracle));
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statement_count, 1,
        "WITH FUNCTION slash must only terminate: {items:?}"
    );
}

#[test]
fn test_slash_does_not_repeat_labeled_oracle_block() {
    let sql = "<<outer_block>>\n\
DECLARE\n\
  value NUMBER := 1;\n\
BEGIN\n\
  NULL;\n\
END outer_block;\n/";
    let items =
        QueryExecutor::split_script_items_for_db_type(sql, Some(crate::db::DatabaseType::Oracle));
    let statement_count = items
        .iter()
        .filter(|item| matches!(item, ScriptItem::Statement(_)))
        .count();

    assert_eq!(
        statement_count, 1,
        "labeled block slash must only terminate: {items:?}"
    );
}

#[test]
fn test_slash_does_not_duplicate_labeled_procedure_end_fragment() {
    let sql = "CREATE OR REPLACE PROCEDURE demo_proc IS\n\
BEGIN\n\
  NULL;\n\
END demo_proc;\n/";
    let items =
        QueryExecutor::split_script_items_for_db_type(sql, Some(crate::db::DatabaseType::Oracle));
    let statements = items
        .iter()
        .filter_map(|item| match item {
            ScriptItem::Statement(statement) => Some(statement.as_str()),
            ScriptItem::ToolCommand(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(statements.len(), 1, "unexpected procedure split: {items:?}");
    assert!(statements[0].contains("END demo_proc"));
}
