use super::SqlEditorWidget;
use crate::db::connection::DatabaseType;

fn format_stable(source: &str, db_type: DatabaseType) -> String {
    let formatted =
        SqlEditorWidget::format_for_auto_formatting_with_db_type(source, false, Some(db_type));
    assert_eq!(
        SqlEditorWidget::format_for_auto_formatting_with_db_type(&formatted, false, Some(db_type),),
        formatted,
        "auto-formatting should be idempotent for {db_type:?}:\n{formatted}"
    );
    formatted
}

fn indent(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn line_starting_with<'a>(formatted: &'a str, prefix: &str) -> &'a str {
    formatted
        .lines()
        .find(|line| line.trim_start().starts_with(prefix))
        .unwrap_or_else(|| panic!("missing line starting with `{prefix}`:\n{formatted}"))
}

#[test]
fn visual_oracle_keeps_exception_declaration_and_outer_begin_on_owner_depth() {
    let source = r#"CREATE OR REPLACE PROCEDURE visual_exception IS
    e_bad EXCEPTION;
    PRAGMA EXCEPTION_INIT (e_bad, -20001);
BEGIN
    NULL;
END visual_exception;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);

    let declaration = line_starting_with(&formatted, "e_bad");
    let outer_begin = formatted
        .lines()
        .find(|line| line.trim() == "BEGIN")
        .expect("procedure BEGIN");
    let outer_end = line_starting_with(&formatted, "END visual_exception;");

    assert_eq!(declaration.trim(), "e_bad EXCEPTION;", "{formatted}");
    assert_eq!(indent(declaration), 4, "{formatted}");
    assert_eq!(indent(outer_begin), 0, "{formatted}");
    assert_eq!(indent(outer_end), 0, "{formatted}");
}

#[test]
fn visual_oracle_keeps_if_cte_and_outer_query_at_root_depth() {
    let source = r#"WITH IF AS (
    SELECT 1 AS id FROM dual
)
SELECT IF.id
FROM IF;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let lines: Vec<&str> = formatted.lines().collect();
    let outer_from_idx = lines
        .iter()
        .position(|line| line.trim() == "FROM IF;")
        .unwrap_or_else(|| panic!("outer FROM IF line:\n{formatted}"));
    let outer_select = lines[..outer_from_idx]
        .iter()
        .rev()
        .find(|line| line.trim_start().starts_with("SELECT IF.id"))
        .unwrap_or_else(|| panic!("outer SELECT IF.id line:\n{formatted}"));

    assert!(formatted.contains("WITH IF AS ("), "{formatted}");
    assert_eq!(indent(outer_select), 0, "{formatted}");
    assert_eq!(indent(lines[outer_from_idx]), 0, "{formatted}");
}

#[test]
fn visual_oracle_treats_if_qualifier_as_identifier_inside_analytic_partition() {
    let formatted = format_stable(
        "SELECT IF.a, IF.grp, ROW_NUMBER() OVER (PARTITION BY IF.grp ORDER BY IF.a) AS rn, SUM(IF.c) OVER (PARTITION BY IF.grp ORDER BY IF.a ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS running_sum FROM qt_if_base IF ORDER BY IF.a;",
        DatabaseType::Oracle,
    );
    let partition_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "PARTITION BY IF.grp")
        .collect();

    assert_eq!(partition_lines.len(), 2, "{formatted}");
    assert!(
        formatted
            .lines()
            .any(|line| line.trim() == "FROM qt_if_base IF"),
        "{formatted}"
    );
    assert!(
        formatted.lines().any(|line| line == "ORDER BY IF.a;"),
        "{formatted}"
    );
}

#[test]
fn visual_oracle_keeps_compound_sql_phrases_together() {
    let source = r#"SELECT * FROM visual_t WHERE nullable_col IS
NULL;
SELECT * FROM visual_t FOR
UPDATE;
SELECT LISTAGG(name, ',') WITHIN
GROUP (ORDER BY name) FROM visual_t;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);

    assert!(formatted.contains("IS NULL"), "{formatted}");
    assert!(formatted.contains("FOR UPDATE"), "{formatted}");
    assert!(formatted.contains("WITHIN GROUP ("), "{formatted}");
    assert!(!formatted.contains("IS\n"), "{formatted}");
    assert!(!formatted.contains("FOR\nUPDATE"), "{formatted}");
    assert!(!formatted.contains("WITHIN\nGROUP"), "{formatted}");

    let analytic = format_stable(
        "SELECT dept_id, LISTAGG(name, ',') WITHIN GROUP (ORDER BY name) OVER (PARTITION BY dept_id) AS names FROM visual_t;",
        DatabaseType::Oracle,
    );
    let within = line_starting_with(&analytic, "LISTAGG");
    let partition = line_starting_with(&analytic, "PARTITION BY dept_id");
    let close = line_starting_with(&analytic, ") AS names");
    assert!(within.contains("WITHIN GROUP"), "{analytic}");
    assert_eq!(indent(partition), indent(within) + 4, "{analytic}");
    assert_eq!(indent(close), indent(within), "{analytic}");
}

#[test]
fn visual_oracle_treats_line_initial_remark_as_an_identifier() {
    let source = r#"CREATE TABLE visual_remark (
    id NUMBER,
remark VARCHAR2(20)
);
INSERT INTO visual_remark (id,
remark
) VALUES (1, 'ok');"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let remark_column = formatted
        .lines()
        .find(|line| {
            line.trim_start()
                .eq_ignore_ascii_case("remark VARCHAR2 (20)")
        })
        .unwrap_or_else(|| panic!("REMARK column declaration:\n{formatted}"));

    assert_eq!(indent(remark_column), 4, "{formatted}");
    let lines: Vec<&str> = formatted.lines().collect();
    let insert_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("INSERT INTO visual_remark"))
        .unwrap_or_else(|| panic!("INSERT using REMARK column:\n{formatted}"));
    if !lines[insert_idx].contains("remark") {
        let insert_remark = lines
            .iter()
            .skip(insert_idx + 1)
            .find(|line| line.trim().eq_ignore_ascii_case("remark"))
            .unwrap_or_else(|| panic!("REMARK insert-list item:\n{formatted}"));
        assert!(
            indent(insert_remark) > indent(lines[insert_idx]),
            "{formatted}"
        );
    }
}

#[test]
fn visual_mysql_profiles_distinguish_repeat_function_from_repeat_loop() {
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let function = format_stable("SELECT REPEAT('ab', 3) AS repeated_value;", db_type);
        assert!(
            function.contains("REPEAT('ab', 3)"),
            "{db_type:?}:\n{function}"
        );
        assert!(
            !function.lines().any(|line| line.trim() == "REPEAT"),
            "{db_type:?}:\n{function}"
        );

        let loop_sql = r#"BEGIN
REPEAT
SET i = i + 1;
UNTIL i >= 3
END REPEAT;
END;"#;
        let repeated = format_stable(loop_sql, db_type);
        let repeat_line = repeated
            .lines()
            .find(|line| line.trim() == "REPEAT")
            .unwrap_or_else(|| panic!("REPEAT header for {db_type:?}:\n{repeated}"));
        let set_line = line_starting_with(&repeated, "SET i = i + 1;");
        let until_line = line_starting_with(&repeated, "UNTIL i >= 3");
        let end_repeat = line_starting_with(&repeated, "END REPEAT;");

        assert_eq!(indent(set_line), indent(repeat_line) + 4, "{repeated}");
        assert_eq!(indent(until_line), indent(repeat_line), "{repeated}");
        assert_eq!(indent(end_repeat), indent(repeat_line), "{repeated}");
    }
}

#[test]
fn visual_oracle_multiline_if_body_uses_one_structural_step() {
    let source = r#"BEGIN
IF l_mode = 'STRICT'
    AND l_count > 0 THEN
BEGIN
NULL;
END;
END IF;
END;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let lines: Vec<&str> = formatted.lines().collect();
    let if_idx = lines
        .iter()
        .position(|line| line.trim() == "IF")
        .unwrap_or_else(|| panic!("IF header:\n{formatted}"));
    let first_condition_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("l_mode = 'STRICT'"))
        .unwrap_or_else(|| panic!("IF first condition:\n{formatted}"));
    let and_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("AND l_count > 0 THEN"))
        .unwrap_or_else(|| panic!("IF continuation:\n{formatted}"));
    let body_begin_idx = lines
        .iter()
        .enumerate()
        .skip(if_idx + 1)
        .find(|(_, line)| line.trim() == "BEGIN")
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| panic!("nested IF body BEGIN:\n{formatted}"));
    let null_idx = lines
        .iter()
        .enumerate()
        .skip(body_begin_idx + 1)
        .find(|(_, line)| line.trim() == "NULL;")
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| panic!("nested IF body statement:\n{formatted}"));

    assert_eq!(
        indent(lines[first_condition_idx]),
        indent(lines[if_idx]) + 4,
        "{formatted}"
    );
    assert_eq!(
        indent(lines[and_idx]),
        indent(lines[first_condition_idx]),
        "{formatted}"
    );
    assert_eq!(
        indent(lines[body_begin_idx]),
        indent(lines[if_idx]) + 4,
        "{formatted}"
    );
    assert_eq!(
        indent(lines[null_idx]),
        indent(lines[body_begin_idx]) + 4,
        "{formatted}"
    );
}

#[test]
fn visual_oracle_return_case_stays_on_return_owner_depth() {
    let source = r#"CREATE OR REPLACE FUNCTION visual_case RETURN NUMBER IS
BEGIN
    RETURN
    CASE
        WHEN 1 = 1 THEN 1
        ELSE 0
    END;
END visual_case;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let return_line = formatted
        .lines()
        .find(|line| line.trim() == "RETURN CASE")
        .unwrap_or_else(|| panic!("RETURN CASE line:\n{formatted}"));
    let case_end = formatted
        .lines()
        .find(|line| line.trim() == "END;")
        .unwrap_or_else(|| panic!("CASE terminator:\n{formatted}"));

    assert_eq!(indent(case_end), indent(return_line), "{formatted}");
}

#[test]
fn visual_mariadb_indents_comment_and_first_nested_select_item() {
    let formatted = format_stable(
        r#"SELECT SUM(v.net_amount)
FROM (
        SELECT
        /* comment before expression */
            calc_net(i.qty) AS net_amount
        FROM visual_order i
    ) v;"#,
        DatabaseType::MariaDB,
    );

    let select = formatted
        .lines()
        .find(|line| line.trim() == "SELECT")
        .unwrap_or_else(|| panic!("nested SELECT:\n{formatted}"));
    let comment = line_starting_with(&formatted, "/* comment before expression */");
    let expression = line_starting_with(&formatted, "calc_net(i.qty)");
    let from = line_starting_with(&formatted, "FROM visual_order i");
    assert_eq!(indent(comment), indent(select) + 4, "{formatted}");
    assert_eq!(indent(expression), indent(comment), "{formatted}");
    assert_eq!(indent(from), indent(select), "{formatted}");
}

#[test]
fn visual_oracle_aligns_commented_inline_view_with_and_main_select() {
    let formatted = format_stable(
        r#"DECLARE
    p_rc SYS_REFCURSOR;
BEGIN
    OPEN p_rc FOR
        WITH paid AS (
            SELECT 1 AS id FROM DUAL
        )
        SELECT *
        FROM (
                /* inline view */
                WITH x AS (
                    SELECT id FROM paid
                )
                SELECT x.* FROM x
            ) v;
END;"#,
        DatabaseType::Oracle,
    );

    let comment = line_starting_with(&formatted, "/* inline view */");
    let with = line_starting_with(&formatted, "WITH x AS (");
    let main_select = line_starting_with(&formatted, "SELECT x.*");
    let cte_select = line_starting_with(&formatted, "SELECT id");
    assert_eq!(indent(comment), indent(main_select), "{formatted}");
    assert_eq!(indent(with), indent(main_select), "{formatted}");
    assert_eq!(indent(cte_select), indent(with) + 8, "{formatted}");
}

#[test]
fn visual_oracle_aligns_case_and_following_within_group_order_items() {
    let formatted = format_stable(
        r#"SELECT LISTAGG(employee_name, ', ') WITHIN GROUP (
    ORDER BY
        CASE
            WHEN salary IS NULL THEN 999999999
            ELSE salary * -1
        END,
        employee_id
) AS employee_names
FROM visual_employee;"#,
        DatabaseType::Oracle,
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let order_index = lines
        .iter()
        .position(|line| line.trim() == "ORDER BY")
        .unwrap_or_else(|| panic!("WITHIN GROUP ORDER BY:\n{formatted}"));
    let case = lines
        .iter()
        .skip(order_index + 1)
        .find(|line| line.trim() == "CASE")
        .copied()
        .unwrap_or_else(|| panic!("ORDER BY CASE:\n{formatted}"));
    let employee_id = line_starting_with(&formatted, "employee_id");
    assert_eq!(indent(case), indent(lines[order_index]) + 4, "{formatted}");
    assert_eq!(indent(employee_id), indent(case), "{formatted}");
}

#[test]
fn visual_oracle_aligns_create_view_query_boundary_comment() {
    let formatted = format_stable(
        r#"CREATE OR REPLACE VIEW visual_employee_v AS
WITH employee_base AS (
    SELECT employee_id FROM visual_employee
)
/* query body */
SELECT employee_id FROM employee_base;"#,
        DatabaseType::Oracle,
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let with_index = lines
        .iter()
        .position(|line| line.trim() == "WITH employee_base AS (")
        .unwrap_or_else(|| panic!("CREATE VIEW WITH:\n{formatted}"));
    let comment_index = lines
        .iter()
        .position(|line| line.trim() == "/* query body */")
        .unwrap_or_else(|| panic!("query body comment:\n{formatted}"));
    let main_select_index = lines
        .iter()
        .enumerate()
        .skip(comment_index + 1)
        .find(|(_, line)| line.trim() == "SELECT employee_id")
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("main SELECT:\n{formatted}"));
    let main_from_index = lines
        .iter()
        .enumerate()
        .skip(main_select_index + 1)
        .find(|(_, line)| line.trim() == "FROM employee_base;")
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("main FROM:\n{formatted}"));
    assert_eq!(indent(lines[with_index]), 4, "{formatted}");
    assert_eq!(
        indent(lines[comment_index]),
        indent(lines[with_index]),
        "{formatted}"
    );
    assert_eq!(
        indent(lines[main_select_index]),
        indent(lines[with_index]),
        "{formatted}"
    );
    assert_eq!(
        indent(lines[main_from_index]),
        indent(lines[main_select_index]),
        "{formatted}"
    );
}

#[test]
fn visual_mysql_profiles_keep_return_case_as_one_expression_phrase() {
    let source = r#"CREATE FUNCTION visual_case() RETURNS INT
BEGIN
    RETURN
    CASE
        WHEN 1 = 1 THEN 1
        ELSE 0
    END;
END"#;
    for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
        let formatted = format_stable(source, db_type);
        assert!(
            formatted.contains("RETURN CASE"),
            "{db_type:?}:\n{formatted}"
        );
        assert!(!formatted.contains("RETURN\n"), "{db_type:?}:\n{formatted}");
    }
}

#[test]
fn visual_oracle_breaks_compound_trigger_and_storage_clause_headers() {
    let trigger = format_stable(
        r#"CREATE OR REPLACE TRIGGER visual_ct
FOR INSERT ON visual_t COMPOUND TRIGGER TYPE id_tab IS TABLE OF NUMBER;
g_ids id_tab;
BEFORE STATEMENT IS
BEGIN
    NULL;
END BEFORE STATEMENT;
END visual_ct;"#,
        DatabaseType::Oracle,
    );
    assert!(
        trigger.contains("ON visual_t\n    COMPOUND TRIGGER\n        TYPE id_tab"),
        "{trigger}"
    );

    let storage = format_stable(
        r#"CREATE TABLE visual_lob (doc CLOB, ndoc NCLOB, xdoc XMLTYPE)
LOB (doc) STORE AS BASICFILE
LOB (ndoc) STORE AS BASICFILE
XMLTYPE COLUMN xdoc STORE AS BASICFILE CLOB;"#,
        DatabaseType::Oracle,
    );
    assert!(
        storage.contains(
            "LOB (doc) STORE AS BASICFILE\nLOB (ndoc) STORE AS BASICFILE\nXMLTYPE COLUMN"
        ),
        "{storage}"
    );
}

#[test]
fn visual_oracle_attaches_comment_following_set_comma_to_next_assignment() {
    let formatted = format_stable(
        "UPDATE visual_t SET first_col = 1\n-- first assignment\n, second_col = 2;",
        DatabaseType::Oracle,
    );
    assert!(
        formatted.contains("-- first assignment\n    , second_col = 2;"),
        "{formatted}"
    );
    assert!(!formatted.contains("\n    ,\n"), "{formatted}");
}

#[test]
fn visual_oracle_keeps_nested_case_and_wrapped_comment_body_depths() {
    let source = r#"BEGIN
    CASE
        WHEN mode_id = 1 THEN
            value_id := 1;
            CASE
                WHEN flag_id = 1 THEN
                    value_id := 2;
                ELSE
                    value_id := 3;
            END CASE;
        ELSE
            NULL;
    END CASE;
END;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let outer_when = line_starting_with(&formatted, "WHEN mode_id = 1 THEN");
    let nested_case = formatted
        .lines()
        .find(|line| line.trim() == "CASE" && indent(line) > indent(outer_when))
        .unwrap_or_else(|| panic!("nested CASE:\n{formatted}"));
    assert_eq!(indent(nested_case), indent(outer_when) + 4, "{formatted}");

    let model = format_stable(
        r#"SELECT dept_id FROM visual_t
MODEL
    DIMENSION BY (dept_id)
    MEASURES (
        /* calculated measure */
        amount, 0 AS projected
    )
    RULES (
        -- projection rule
        projected[ANY] = amount[CV()]
    );"#,
        DatabaseType::Oracle,
    );
    let measures = line_starting_with(&model, "MEASURES (");
    let measure_comment = line_starting_with(&model, "/* calculated measure */");
    let rule_comment = line_starting_with(&model, "-- projection rule");
    assert_eq!(indent(measure_comment), indent(measures) + 4, "{model}");
    assert!(indent(rule_comment) > 0, "{model}");
}

#[test]
fn visual_all_profiles_trim_line_ends_and_distinguish_unary_minus() {
    let source = "SELECT -1 AS negative_value, 2 - 1 AS difference  \nFROM visual_t; \t\n";

    for db_type in [
        DatabaseType::Oracle,
        DatabaseType::MySQL,
        DatabaseType::MariaDB,
    ] {
        let formatted = format_stable(source, db_type);
        assert!(formatted.contains("-1 AS negative_value"), "{formatted}");
        assert!(formatted.contains("2 - 1 AS difference"), "{formatted}");
        assert!(!formatted.contains("- 1 AS negative_value"), "{formatted}");
        assert!(
            formatted
                .lines()
                .all(|line| !line.ends_with(' ') && !line.ends_with('\t')),
            "trailing whitespace for {db_type:?}:\n{formatted}"
        );
    }
}

#[test]
fn visual_oracle_keeps_match_recognize_quantifiers_attached() {
    let source = r#"SELECT * FROM visual_sales
MATCH_RECOGNIZE (
    ORDER BY sale_date
    PATTERN (A B+ C*)
    DEFINE A AS amount < 10
);"#;
    let formatted = format_stable(source, DatabaseType::Oracle);

    assert!(formatted.contains("PATTERN (A B+ C*)"), "{formatted}");
    assert!(!formatted.contains("B +"), "{formatted}");
    assert!(!formatted.contains("C *"), "{formatted}");
}

#[test]
fn visual_oracle_keeps_plsql_label_on_its_own_line() {
    let source = r#"BEGIN
    <<top>>
    l_count := l_count + 1;
    IF l_count < 2 THEN
        GOTO top;
    END IF;
END;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let lines: Vec<&str> = formatted.lines().collect();
    let label_idx = lines
        .iter()
        .position(|line| line.trim() == "<<top>>")
        .unwrap_or_else(|| panic!("standalone label:\n{formatted}"));
    let statement_idx = lines
        .iter()
        .position(|line| line.trim() == "l_count := l_count + 1;")
        .unwrap_or_else(|| panic!("statement after label:\n{formatted}"));

    assert_eq!(statement_idx, label_idx + 1, "{formatted}");
    assert_eq!(
        indent(lines[statement_idx]),
        indent(lines[label_idx]),
        "{formatted}"
    );
    assert!(!formatted.contains(">> l_count"), "{formatted}");
}

#[test]
fn visual_oracle_comment_and_outer_from_return_to_query_depth() {
    let source = r#"SELECT
    -- correlated aggregate
    (
        SELECT MAX(i.metric)
        FROM visual_inner i
        WHERE i.id = o.id
    ) AS max_metric,
    o.id
FROM visual_outer o;"#;
    let formatted = format_stable(source, DatabaseType::Oracle);
    let comment = line_starting_with(&formatted, "-- correlated aggregate");
    let sibling = line_starting_with(&formatted, "o.id");
    let inner_from = line_starting_with(&formatted, "FROM visual_inner i");
    let outer_from = line_starting_with(&formatted, "FROM visual_outer o;");

    assert_eq!(indent(comment), indent(outer_from) + 4, "{formatted}");
    assert_eq!(indent(sibling), indent(outer_from) + 4, "{formatted}");
    assert!(indent(inner_from) > indent(outer_from), "{formatted}");
    assert_eq!(indent(outer_from), 0, "{formatted}");
}

#[test]
fn visual_oracle_aligns_commented_query_pivot_and_apply_siblings() {
    let scalar = format_stable(
        r#"SELECT (
    /* correlated aggregate */
    SELECT MAX(i.metric)
    FROM visual_inner i
    WHERE i.id = o.id
) AS max_metric
FROM visual_outer o;"#,
        DatabaseType::Oracle,
    );
    let scalar_comment = line_starting_with(&scalar, "/* correlated aggregate */");
    let scalar_select = line_starting_with(&scalar, "SELECT MAX (i.metric)");
    let scalar_from = line_starting_with(&scalar, "FROM visual_inner i");
    let scalar_where = line_starting_with(&scalar, "WHERE i.id = o.id");
    for sibling in [scalar_select, scalar_from, scalar_where] {
        assert_eq!(indent(sibling), indent(scalar_comment), "{scalar}");
    }

    let pivot = format_stable(
        r#"SELECT * FROM visual_src
PIVOT (
    /* aggregate */
    SUM(amount) AS total_amount
    FOR category IN ('A' AS a)
);"#,
        DatabaseType::Oracle,
    );
    let pivot_owner = line_starting_with(&pivot, "PIVOT (");
    let pivot_comment = line_starting_with(&pivot, "/* aggregate */");
    let pivot_sum = line_starting_with(&pivot, "SUM (amount)");
    let pivot_for = line_starting_with(&pivot, "FOR category IN");
    assert_eq!(indent(pivot_comment), indent(pivot_owner) + 4, "{pivot}");
    assert_eq!(indent(pivot_sum), indent(pivot_comment), "{pivot}");
    assert_eq!(indent(pivot_for), indent(pivot_comment), "{pivot}");

    let apply = format_stable(
        r#"SELECT * FROM visual_src s
CROSS APPLY (
    -- aggregate
    SELECT COUNT(*) AS item_count
    FROM visual_item i
    WHERE i.id = s.id
) a;"#,
        DatabaseType::Oracle,
    );
    let apply_comment = line_starting_with(&apply, "-- aggregate");
    for prefix in [
        "SELECT COUNT(*) AS item_count",
        "FROM visual_item i",
        "WHERE i.id = s.id",
    ] {
        assert_eq!(
            indent(line_starting_with(&apply, prefix)),
            indent(apply_comment),
            "{apply}"
        );
    }
}

#[test]
fn visual_oracle_aligns_plsql_comments_nested_blocks_and_member_calls() {
    let formatted = format_stable(
        r#"BEGIN
    CASE mode_id
        WHEN 1 THEN NULL;
        ELSE
            -- loop body
            FOR i IN 1..2 LOOP
                NULL;
            END LOOP;
    END CASE;
    BEGIN
        -- nested body
        g_ids.DELETE;
    EXCEPTION
        WHEN OTHERS THEN
            NULL;
    END;
END;"#,
        DatabaseType::Oracle,
    );
    let loop_comment = line_starting_with(&formatted, "-- loop body");
    let loop_line = line_starting_with(&formatted, "FOR i IN 1..2 LOOP");
    let nested_begin = formatted
        .lines()
        .filter(|line| line.trim() == "BEGIN")
        .nth(1)
        .unwrap_or_else(|| panic!("nested BEGIN:\n{formatted}"));
    let nested_comment = line_starting_with(&formatted, "-- nested body");
    let member_call = line_starting_with(&formatted, "g_ids.DELETE;");
    let exception = line_starting_with(&formatted, "EXCEPTION");
    let when = line_starting_with(&formatted, "WHEN OTHERS THEN");

    assert_eq!(indent(loop_comment), indent(loop_line), "{formatted}");
    assert_eq!(
        indent(nested_comment),
        indent(nested_begin) + 4,
        "{formatted}"
    );
    assert_eq!(indent(member_call), indent(nested_comment), "{formatted}");
    assert_eq!(indent(exception), indent(nested_begin), "{formatted}");
    assert_eq!(indent(when), indent(exception) + 4, "{formatted}");
}

#[test]
fn visual_oracle_aligns_insert_all_sibling_branches() {
    let formatted = format_stable(
        r#"INSERT ALL
WHEN dept_id = 10 AND salary > 100 THEN
INTO visual_high (id) VALUES (id)
WHEN dept_id = 10 AND salary <= 100 THEN
INTO visual_low (id) VALUES (id)
SELECT id, dept_id, salary FROM visual_emp;"#,
        DatabaseType::Oracle,
    );
    let when_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "WHEN")
        .collect();
    let condition_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("dept_id = 10"))
        .collect();
    let into_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("INTO visual_"))
        .collect();
    assert_eq!(when_lines.len(), 2, "{formatted}");
    assert_eq!(condition_lines.len(), 2, "{formatted}");
    assert_eq!(into_lines.len(), 2, "{formatted}");
    assert_eq!(indent(when_lines[0]), indent(when_lines[1]), "{formatted}");
    assert_eq!(
        indent(condition_lines[0]),
        indent(when_lines[0]) + 4,
        "{formatted}"
    );
    assert_eq!(indent(into_lines[0]), indent(into_lines[1]), "{formatted}");
    assert_eq!(
        indent(into_lines[0]),
        indent(when_lines[0]) + 4,
        "{formatted}"
    );
}

#[test]
fn visual_oracle_aligns_case_results_and_first_select_case_item() {
    let formatted = format_stable(
        r#"BEGIN
    v_ratio := CASE
        WHEN target_value IS NULL
            OR target_value = 0 THEN NULL
        WHEN current_value IS NULL THEN 0
        ELSE ROUND(current_value / target_value, 2)
    END;
END;
UPDATE visual_emp e
SET e.comm = (
    SELECT
        CASE WHEN AVG(x.sal) > 100 THEN 10 ELSE 0 END
    FROM visual_emp x
);"#,
        DatabaseType::Oracle,
    );
    let null_result = line_starting_with(&formatted, "NULL");
    let zero_result = line_starting_with(&formatted, "0");
    let round_result = line_starting_with(&formatted, "ROUND (current_value");
    assert_eq!(indent(null_result), indent(zero_result), "{formatted}");
    assert_eq!(indent(round_result), indent(zero_result), "{formatted}");

    let select = formatted
        .lines()
        .find(|line| line.trim() == "SELECT")
        .unwrap_or_else(|| panic!("SELECT owner:\n{formatted}"));
    let select_case = formatted
        .lines()
        .find(|line| line.trim() == "CASE" && indent(line) > indent(select))
        .unwrap_or_else(|| panic!("SELECT CASE item:\n{formatted}"));
    assert_eq!(indent(select_case), indent(select) + 4, "{formatted}");
}

#[test]
fn visual_oracle_keeps_case_branch_dml_clauses_on_the_statement_depth() {
    let formatted = format_stable(
        r#"BEGIN
    CASE p_action
        WHEN 'BONUS' THEN
            UPDATE visual_employee
            SET salary = salary + 1
            WHERE employee_id = p_id;
        WHEN 'AUDIT' THEN
            INSERT INTO visual_audit (employee_id, action)
            VALUES (p_id, 'PROCESSED');
        ELSE
            NULL;
    END CASE;
END;"#,
        DatabaseType::Oracle,
    );

    let update = line_starting_with(&formatted, "UPDATE visual_employee");
    let set = line_starting_with(&formatted, "SET salary");
    let where_clause = line_starting_with(&formatted, "WHERE employee_id");
    let insert = line_starting_with(&formatted, "INSERT INTO visual_audit");
    let values = line_starting_with(&formatted, "VALUES (p_id");
    assert_eq!(indent(set), indent(update), "{formatted}");
    assert_eq!(indent(where_clause), indent(update), "{formatted}");
    assert_eq!(indent(values), indent(insert), "{formatted}");
}

#[test]
fn visual_sqlplus_mixed_line_preserves_all_source_tokens() {
    let formatted = format_stable(
        "SET PAGESIZE 50 WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK PROMPT creating CREATE TABLE visual_sqlplus (\nid NUMBER\n);",
        DatabaseType::Oracle,
    );
    for token in [
        "WHENEVER SQLERROR",
        "PROMPT creating",
        "CREATE TABLE visual_sqlplus",
    ] {
        assert!(formatted.contains(token), "missing `{token}`:\n{formatted}");
    }
    assert!(
        formatted.contains("SET PAGESIZE 50\n\nWHENEVER SQLERROR"),
        "SET/WHENEVER boundary:\n{formatted}"
    );
    assert!(
        formatted.contains("PROMPT creating\n\nCREATE TABLE visual_sqlplus ("),
        "PROMPT/CREATE boundary:\n{formatted}"
    );
    assert!(
        formatted.contains("CREATE TABLE visual_sqlplus (\n    id NUMBER\n);"),
        "CREATE TABLE body:\n{formatted}"
    );

    let slash_column = format_stable(
        "BEGIN\nNULL;\nEND;\n/ COLUMN id FORMAT 9999 COLUMN data FORMAT A30\nSELECT id, data FROM visual_sqlplus;",
        DatabaseType::Oracle,
    );
    assert!(
        slash_column.contains("END;\n/\n\nCOLUMN id FORMAT 9999\n\nCOLUMN data FORMAT A30"),
        "slash/COLUMN boundaries:\n{slash_column}"
    );
}

#[test]
fn visual_oracle_keeps_named_argument_case_attached_and_splits_the_next_argument() {
    let formatted = format_stable(
        r#"BEGIN
    mutate_emp (
        p_emp_id => r.emp_id,
        p_status => CASE WHEN MOD (r.emp_id, 3) = 0 THEN 'ON_HOLD' ELSE 'ACTIVE' END,
        p_note => v_note
    );
END;"#,
        DatabaseType::Oracle,
    );

    assert!(formatted.contains("p_status => CASE"), "{formatted}");
    assert!(!formatted.contains("p_status =>\n"), "{formatted}");

    let case_end = formatted
        .lines()
        .find(|line| line.trim() == "END,")
        .unwrap_or_else(|| panic!("named-argument CASE terminator:\n{formatted}"));
    let next_argument = line_starting_with(&formatted, "p_note => v_note");
    assert_eq!(indent(next_argument), indent(case_end), "{formatted}");
}

#[test]
fn visual_oracle_indents_execute_immediate_case_expression_from_its_owner() {
    let formatted = format_stable(
        r#"BEGIN
    EXECUTE IMMEDIATE
        CASE
            WHEN p_kind = 'TABLE' THEN 'DROP TABLE ' || p_name
            ELSE NULL
        END;
END;"#,
        DatabaseType::Oracle,
    );

    let execute = line_starting_with(&formatted, "EXECUTE IMMEDIATE");
    let case = line_starting_with(&formatted, "CASE");
    let case_end = line_starting_with(&formatted, "END;");
    assert_eq!(indent(case), indent(execute) + 4, "{formatted}");
    assert_eq!(indent(case_end), indent(case), "{formatted}");
}

#[test]
fn visual_oracle_splits_multiline_execute_immediate_using_bind_arguments() {
    let formatted = format_stable(
        r#"BEGIN
    EXECUTE IMMEDIATE v_sql USING visual_seq.NEXTVAL, CASE MOD (v_idx, 2) WHEN 0 THEN 10 ELSE 20 END, v_name, CASE WHEN v_active = 1 THEN 'Y' ELSE 'N' END, ROUND (v_amount, 2), v_note;
END;"#,
        DatabaseType::Oracle,
    );

    let execute = line_starting_with(&formatted, "EXECUTE IMMEDIATE");
    let case_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("CASE"))
        .collect();
    let case_end_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "END,")
        .collect();
    let v_name = line_starting_with(&formatted, "v_name,");
    let round = line_starting_with(&formatted, "ROUND (v_amount, 2),");
    let v_note = line_starting_with(&formatted, "v_note;");

    assert_eq!(case_lines.len(), 2, "{formatted}");
    assert_eq!(case_end_lines.len(), 2, "{formatted}");
    for line in case_lines
        .into_iter()
        .chain(case_end_lines)
        .chain([v_name, round, v_note])
    {
        assert_eq!(indent(line), indent(execute) + 4, "{formatted}");
    }
}

#[test]
fn visual_oracle_aligns_conditional_insert_all_and_driving_select() {
    let formatted = format_stable(
        r#"INSERT ALL
    WHEN dept_id = 30 AND salary >= 100000 THEN
        INTO visual_high (id) VALUES (id)
    WHEN dept_id = 30 AND salary < 100000 THEN
        INTO visual_low (id) VALUES (id)
SELECT id, dept_id, salary
FROM visual_emp;"#,
        DatabaseType::Oracle,
    );

    let insert_all = line_starting_with(&formatted, "INSERT ALL");
    let when = formatted
        .lines()
        .find(|line| line.trim() == "WHEN")
        .expect("conditional INSERT WHEN header");
    let first_condition = line_starting_with(&formatted, "dept_id = 30");
    let and = line_starting_with(&formatted, "AND salary");
    let select = line_starting_with(&formatted, "SELECT id");
    let from = line_starting_with(&formatted, "FROM visual_emp");
    assert_eq!(indent(first_condition), indent(when) + 4, "{formatted}");
    assert_eq!(indent(and), indent(first_condition), "{formatted}");
    assert_eq!(indent(select), indent(insert_all), "{formatted}");
    assert_eq!(indent(from), indent(insert_all), "{formatted}");
}

#[test]
fn visual_oracle_indents_case_used_as_a_for_range_expression() {
    let formatted = format_stable(
        r#"BEGIN
    FOR i IN 1..
        CASE
            WHEN p_limit IS NULL THEN 5
            ELSE p_limit
        END LOOP
        NULL;
    END LOOP;
END;"#,
        DatabaseType::Oracle,
    );

    let loop_header = line_starting_with(&formatted, "FOR i IN 1..");
    let case = line_starting_with(&formatted, "CASE");
    let case_end = formatted
        .lines()
        .find(|line| line.trim() == "END")
        .expect("FOR upper-bound CASE END");
    let loop_open = formatted
        .lines()
        .find(|line| line.trim() == "LOOP")
        .expect("FOR LOOP opener after CASE END");
    let body = formatted
        .lines()
        .find(|line| line.trim() == "NULL;")
        .expect("FOR body");
    let loop_end = formatted
        .lines()
        .find(|line| line.trim() == "END LOOP;")
        .expect("FOR END LOOP");
    assert_eq!(indent(case), indent(loop_header) + 4, "{formatted}");
    assert_eq!(indent(case_end), indent(case), "{formatted}");
    assert_eq!(indent(loop_open), indent(loop_header), "{formatted}");
    assert_eq!(indent(body), indent(loop_header) + 4, "{formatted}");
    assert_eq!(indent(loop_end), indent(loop_header), "{formatted}");
}

#[test]
fn visual_oracle_indents_for_loop_body_inside_exception_handler() {
    let formatted = format_stable(
        r#"BEGIN
    BEGIN
        NULL;
    EXCEPTION
        WHEN OTHERS THEN
            FOR j IN 1..3 LOOP
            INSERT INTO visual_errors (id) VALUES (j);
            END LOOP;
    END;
END;"#,
        DatabaseType::Oracle,
    );

    let loop_header = line_starting_with(&formatted, "FOR j IN 1..3 LOOP");
    let body = line_starting_with(&formatted, "INSERT INTO visual_errors");
    let values = line_starting_with(&formatted, "VALUES (j)");
    let loop_end = formatted
        .lines()
        .find(|line| line.trim() == "END LOOP;")
        .expect("FOR END LOOP");
    assert_eq!(indent(body), indent(loop_header) + 4, "{formatted}");
    assert_eq!(indent(values), indent(body), "{formatted}");
    assert_eq!(indent(loop_end), indent(loop_header), "{formatted}");
}

#[test]
fn visual_oracle_keeps_cursor_for_update_inside_the_loop_header() {
    let formatted = format_stable(
        r#"BEGIN
    FOR r IN (
        SELECT id
        FROM visual_emp
        FOR UPDATE OF salary, status
    ) LOOP
        BEGIN
            NULL;
        END;
    END LOOP;
END;"#,
        DatabaseType::Oracle,
    );

    let lines: Vec<&str> = formatted.lines().collect();
    let header_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("FOR r IN ("))
        .expect("cursor FOR header");
    let line_after = |text: &str| {
        lines[header_idx + 1..]
            .iter()
            .copied()
            .find(|line| line.trim() == text)
            .unwrap_or_else(|| panic!("missing `{text}` after cursor FOR:\n{formatted}"))
    };
    let loop_header = lines[header_idx];
    let for_update = line_after("FOR UPDATE OF salary,");
    let loop_open = line_after(") LOOP");
    let body_begin = line_after("BEGIN");
    let body = line_after("NULL;");
    let body_end = line_after("END;");
    let loop_end = line_after("END LOOP;");

    assert_eq!(indent(for_update), indent(loop_header) + 4, "{formatted}");
    assert_eq!(indent(loop_open), indent(loop_header), "{formatted}");
    assert_eq!(indent(body_begin), indent(loop_header) + 4, "{formatted}");
    assert_eq!(indent(body), indent(body_begin) + 4, "{formatted}");
    assert_eq!(indent(body_end), indent(body_begin), "{formatted}");
    assert_eq!(indent(loop_end), indent(loop_header), "{formatted}");
}

#[test]
fn visual_oracle_splits_model_rule_siblings_after_case() {
    let formatted = format_stable(
        r#"SELECT dept_id, month_no, forecast, adjusted
FROM visual_sales
MODEL
    PARTITION BY (dept_id)
    DIMENSION BY (month_no)
    MEASURES (amount, 0 AS forecast, 0 AS adjusted)
    RULES SEQUENTIAL ORDER (
        forecast[ANY, ANY] = CASE WHEN amount[CV(), CV(month_no)] > 0 THEN amount[CV(), CV(month_no)] ELSE 0 END,
        adjusted[ANY, ANY] = CASE WHEN forecast[CV(), CV(month_no)] > 0 THEN forecast[CV(), CV(month_no)] ELSE 0 END
    );"#,
        DatabaseType::Oracle,
    );

    let first_rule = line_starting_with(&formatted, "forecast [ ANY, ANY ] =");
    let second_rule = line_starting_with(&formatted, "adjusted [ ANY, ANY ] =");
    assert_eq!(indent(second_rule), indent(first_rule), "{formatted}");
    assert!(!formatted.contains("END, adjusted"), "{formatted}");
    assert!(!formatted.contains("CV (),\n"), "{formatted}");
}

#[test]
fn visual_oracle_keeps_model_iterate_and_until_modifier_parens_compact() {
    let formatted = format_stable(
        r#"SELECT dept_id, month_no, forecast
FROM visual_sales
MODEL
    PARTITION BY (dept_id)
    DIMENSION BY (month_no)
    MEASURES (amount, 0 AS forecast)
    RULES ITERATE (5) UNTIL (CV(month_no) > 12) (
        forecast[ANY] = amount[CV()]
    );"#,
        DatabaseType::Oracle,
    );

    assert!(
        formatted.contains("RULES ITERATE (5) UNTIL (CV (month_no) > 12)"),
        "{formatted}"
    );
}

#[test]
fn visual_mariadb_indents_nested_or_inside_procedure_if_parentheses() {
    let formatted = format_stable(
        r#"CREATE PROCEDURE visual_validate()
BEGIN
    IF v_orders_cnt > 0 AND (v_top_category IS NULL OR v_top_category = '') THEN
        SET v_message = 'missing category';
    END IF;
END;"#,
        DatabaseType::MariaDB,
    );

    let if_line = formatted
        .lines()
        .find(|line| line.trim() == "IF")
        .expect("procedure IF header");
    let first_condition = line_starting_with(&formatted, "v_orders_cnt > 0");
    let and_line = line_starting_with(&formatted, "AND (v_top_category IS NULL");
    let or_line = line_starting_with(&formatted, "OR v_top_category = '') THEN");
    let body = line_starting_with(&formatted, "SET v_message = 'missing category';");

    assert_eq!(indent(first_condition), indent(if_line) + 4, "{formatted}");
    assert_eq!(indent(and_line), indent(first_condition), "{formatted}");
    assert_eq!(indent(or_line), indent(and_line) + 4, "{formatted}");
    assert_eq!(indent(body), indent(and_line), "{formatted}");
}

#[test]
fn visual_oracle_keeps_model_for_cell_reference_inline() {
    let formatted = format_stable(
        r#"SELECT dept_id, month_no, total_amt
FROM visual_sales
MODEL
    PARTITION BY (dept_id)
    DIMENSION BY (month_no)
    MEASURES (total_amt)
    RULES UPSERT (total_amt [ FOR month_no FROM 1 TO 12 INCREMENT 1 ] = NVL (total_amt [ CV (month_no) ], 0));"#,
        DatabaseType::Oracle,
    );

    assert!(
        formatted.contains(
            "total_amt [ FOR month_no FROM 1 TO 12 INCREMENT 1 ] = NVL (total_amt [ CV (month_no) ], 0)"
        ),
        "{formatted}"
    );
}

#[test]
fn visual_oracle_keeps_empty_analytic_over_parentheses_compact() {
    let formatted = format_stable(
        "SELECT employee_id, COUNT(*) OVER () AS total_cnt FROM visual_employee;",
        DatabaseType::Oracle,
    );

    assert!(
        formatted.contains("COUNT(*) OVER () AS total_cnt"),
        "{formatted}"
    );
}

#[test]
fn visual_oracle_aligns_trailing_cte_query_comment_with_query_body() {
    let formatted = format_stable(
        r#"CREATE OR REPLACE VIEW visual_comment_v AS
WITH base AS (
    SELECT d.id
    FROM visual_data d
    /* trailing query comment */
)
SELECT id
FROM base;"#,
        DatabaseType::Oracle,
    );

    let from = line_starting_with(&formatted, "FROM visual_data d");
    let comment = line_starting_with(&formatted, "/* trailing query comment */");
    assert_eq!(indent(comment), indent(from) + 4, "{formatted}");
}

#[test]
fn visual_oracle_indents_following_analytic_order_key_below_order_by() {
    let formatted = format_stable(
        "SELECT ROW_NUMBER() OVER (PARTITION BY dept_id, team_id ORDER BY salary DESC, employee_id) AS rn FROM visual_employee;",
        DatabaseType::Oracle,
    );

    let partition_by = line_starting_with(&formatted, "PARTITION BY dept_id,");
    let team_id = line_starting_with(&formatted, "team_id");
    let order_by = line_starting_with(&formatted, "ORDER BY salary DESC,");
    let employee_id = line_starting_with(&formatted, "employee_id");
    assert_eq!(indent(team_id), indent(partition_by) + 4, "{formatted}");
    assert_eq!(indent(employee_id), indent(order_by) + 4, "{formatted}");
}

#[test]
fn visual_oracle_keeps_analytic_order_by_scalar_subquery_clauses_multiline() {
    let formatted = format_stable(
        r#"SELECT DENSE_RANK() OVER (
    PARTITION BY e.dept_id
    ORDER BY (
        SELECT SUM(b.amount)
        FROM visual_bonus b
        WHERE b.employee_id = e.employee_id
    ) DESC NULLS LAST
) AS bonus_rank
FROM visual_employee e;"#,
        DatabaseType::Oracle,
    );

    let order_by = line_starting_with(&formatted, "ORDER BY (");
    let select = line_starting_with(&formatted, "SELECT SUM");
    let from = line_starting_with(&formatted, "FROM visual_bonus b");
    let where_clause = line_starting_with(&formatted, "WHERE b.employee_id");

    assert!(indent(select) > indent(order_by), "{formatted}");
    assert_eq!(indent(from), indent(select), "{formatted}");
    assert_eq!(indent(where_clause), indent(select), "{formatted}");
}

#[test]
fn visual_mysql_indents_following_named_window_order_key_below_order_by() {
    let formatted = format_stable(
        "SELECT ROW_NUMBER() OVER visual_window AS rn FROM visual_employee WINDOW visual_window AS (PARTITION BY dept_id, team_id ORDER BY salary DESC, employee_id);",
        DatabaseType::MySQL,
    );

    let partition_by = line_starting_with(&formatted, "PARTITION BY dept_id,");
    let team_id = line_starting_with(&formatted, "team_id");
    let order_by = line_starting_with(&formatted, "ORDER BY salary DESC,");
    let employee_id = line_starting_with(&formatted, "employee_id");
    assert_eq!(indent(team_id), indent(partition_by) + 4, "{formatted}");
    assert_eq!(indent(employee_id), indent(order_by) + 4, "{formatted}");
}

#[test]
fn visual_oracle_breaks_bulk_collect_after_a_multiline_select_list() {
    let formatted = format_stable(
        r#"BEGIN
    SELECT employee_id,
        NVL (bonus, 0) +
        CASE
            WHEN status = 'ACTIVE' THEN 11
            ELSE 7
        END BULK COLLECT INTO employee_ids,
        bonuses
    FROM visual_employee;
END;"#,
        DatabaseType::Oracle,
    );

    let select = line_starting_with(&formatted, "SELECT employee_id,");
    let bulk = line_starting_with(&formatted, "BULK COLLECT INTO employee_ids,");
    let bonuses = line_starting_with(&formatted, "bonuses");
    assert_eq!(indent(bulk), indent(select), "{formatted}");
    assert_eq!(indent(bonuses), indent(bulk) + 4, "{formatted}");
    assert!(!formatted.contains("END BULK COLLECT"), "{formatted}");
}

#[test]
fn visual_oracle_aligns_case_results_after_multiline_when_conditions() {
    let formatted = format_stable(
        r#"CREATE FUNCTION visual_classify(p_salary NUMBER, p_bonus NUMBER) RETURN VARCHAR2 IS
BEGIN
    RETURN CASE
        WHEN p_salary >= 4000 AND NVL (p_bonus, 0) > 0 THEN 'TOP_PLUS_BONUS'
        WHEN p_salary >= 3000 THEN 'TOP'
        ELSE 'OTHER'
    END;
END;"#,
        DatabaseType::Oracle,
    );

    let first_result = line_starting_with(&formatted, "'TOP_PLUS_BONUS'");
    let second_result = line_starting_with(&formatted, "'TOP'");
    let else_result = line_starting_with(&formatted, "'OTHER'");
    assert_eq!(indent(first_result), indent(second_result), "{formatted}");
    assert_eq!(indent(first_result), indent(else_result), "{formatted}");
}

#[test]
fn visual_oracle_splits_merge_insert_and_values_clauses() {
    let formatted = format_stable(
        "MERGE INTO visual_target t USING visual_source s ON (t.id = s.id) WHEN NOT MATCHED THEN INSERT (id, emp_name, created_at) VALUES (s.id, s.emp_name, SYSTIMESTAMP);",
        DatabaseType::Oracle,
    );

    let insert = line_starting_with(&formatted, "INSERT (id, emp_name, created_at)");
    let values = line_starting_with(&formatted, "VALUES (s.id, s.emp_name, SYSTIMESTAMP);");
    assert_eq!(indent(values), indent(insert), "{formatted}");
    assert!(!insert.contains("VALUES"), "{formatted}");
}

#[test]
fn visual_oracle_virtual_column_does_not_pad_other_column_constraints() {
    let formatted = format_stable(
        r#"CREATE TABLE visual_virtual_column (
    id NUMBER NOT NULL,
    total_amount AS (ROUND (id * 11 * 12 * 13, 2))
);"#,
        DatabaseType::Oracle,
    );

    let id = line_starting_with(&formatted, "id");
    let virtual_column = line_starting_with(&formatted, "total_amount");
    assert!(id.contains("NUMBER NOT NULL"), "{formatted}");
    assert!(
        virtual_column.contains("AS (ROUND (id * 11 * 12 * 13, 2))"),
        "{formatted}"
    );
}

#[test]
fn visual_oracle_expands_alter_table_split_partition_destination_list() {
    let formatted = format_stable(
        "ALTER TABLE orders SPLIT PARTITION orders_2024 INTO (PARTITION orders_2024_h1 VALUES LESS THAN (TO_DATE('2024-07-01', 'YYYY-MM-DD')), PARTITION orders_2024_h2 VALUES LESS THAN (TO_DATE('2025-01-01', 'YYYY-MM-DD')));",
        DatabaseType::Oracle,
    );
    let expected = r#"ALTER TABLE orders SPLIT PARTITION orders_2024
INTO (
        PARTITION orders_2024_h1
        VALUES LESS THAN (TO_DATE ('2024-07-01', 'YYYY-MM-DD')),
        PARTITION orders_2024_h2
        VALUES LESS THAN (TO_DATE ('2025-01-01', 'YYYY-MM-DD'))
    );"#;

    assert_eq!(formatted, expected);
}

#[test]
fn visual_oracle_splits_conditional_compilation_directives_from_branch_bodies() {
    let formatted = format_stable(
        r#"CREATE OR REPLACE PROCEDURE visual_cc IS
BEGIN
    $IF DBMS_DB_VERSION.VERSION >= 12 $THEN AUDIT ('cc', 'enabled');
    SELECT $ERROR INTO v_marker FROM dual;
    AUDIT ('cc', 'outer-tail');
    $IF $$debug_mode $THEN AUDIT ('cc', 'nested-enabled');
    $ELSE AUDIT ('cc', 'nested-disabled');
    $END AUDIT ('cc', 'after-inner');
    $ELSE AUDIT ('cc', 'disabled');
    $END AUDIT ('cc', 'done');
END visual_cc;"#,
        DatabaseType::Oracle,
    );

    let if_directives: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("$IF"))
        .collect();
    let else_directives: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "$ELSE")
        .collect();
    let end_directives: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "$END")
        .collect();
    let enabled = line_starting_with(&formatted, "AUDIT ('cc', 'enabled')");
    let error_identifier = line_starting_with(&formatted, "SELECT $ERROR");
    let outer_tail = line_starting_with(&formatted, "AUDIT ('cc', 'outer-tail')");
    let nested_enabled = line_starting_with(&formatted, "AUDIT ('cc', 'nested-enabled')");
    let nested_disabled = line_starting_with(&formatted, "AUDIT ('cc', 'nested-disabled')");
    let after_inner = line_starting_with(&formatted, "AUDIT ('cc', 'after-inner')");
    let disabled = line_starting_with(&formatted, "AUDIT ('cc', 'disabled')");
    let done = line_starting_with(&formatted, "AUDIT ('cc', 'done')");

    assert_eq!(if_directives.len(), 2, "{formatted}");
    assert_eq!(else_directives.len(), 2, "{formatted}");
    assert_eq!(end_directives.len(), 2, "{formatted}");
    assert_eq!(indent(if_directives[0]), 4, "{formatted}");
    assert_eq!(indent(if_directives[1]), 8, "{formatted}");
    assert_eq!(indent(else_directives[0]), 8, "{formatted}");
    assert_eq!(indent(end_directives[0]), 8, "{formatted}");
    assert_eq!(indent(else_directives[1]), 4, "{formatted}");
    assert_eq!(indent(end_directives[1]), 4, "{formatted}");
    assert_eq!(indent(enabled), 8, "{formatted}");
    assert_eq!(error_identifier.trim(), "SELECT $ERROR", "{formatted}");
    assert_eq!(indent(outer_tail), 8, "{formatted}");
    assert_eq!(indent(nested_enabled), 12, "{formatted}");
    assert_eq!(indent(nested_disabled), 12, "{formatted}");
    assert_eq!(indent(after_inner), 8, "{formatted}");
    assert_eq!(indent(disabled), 8, "{formatted}");
    assert_eq!(indent(done), 4, "{formatted}");

    let identifier = format_stable(
        "SELECT $IF AS if_col, $THEN AS then_col FROM dual;",
        DatabaseType::Oracle,
    );
    assert!(
        line_starting_with(&identifier, "SELECT").contains("$IF AS if_col"),
        "{identifier}"
    );
    assert_eq!(
        line_starting_with(&identifier, "$THEN AS then_col").trim(),
        "$THEN AS then_col",
        "{identifier}"
    );
}

#[test]
fn visual_oracle_expands_multiline_xmltable_and_json_table_clauses() {
    let formatted = format_stable(
        r#"SELECT x.emp_id, x.emp_name
FROM XMLTABLE (
    '/rows/row'
    PASSING XMLTYPE (
        '<rows>
            <row><emp_id>1</emp_id><emp_name>ALICE</emp_name></row>
         </rows>'
    )
    COLUMNS
        emp_id NUMBER PATH 'emp_id',
        emp_name VARCHAR2(100) PATH 'emp_name'
) x;

SELECT j.emp_id, j.emp_name
FROM JSON_TABLE (
    '{
       "employees": [{ "id": 1, "name": "ALICE" }]
     }',
    '$.employees[*]'
    COLUMNS
        emp_id NUMBER PATH '$.id',
        emp_name VARCHAR2(100) PATH '$.name'
) j;"#,
        DatabaseType::Oracle,
    );

    let xml_owner = line_starting_with(&formatted, "FROM XMLTABLE (");
    let passing = line_starting_with(&formatted, "PASSING XMLTYPE (");
    let json_owner = line_starting_with(&formatted, "FROM JSON_TABLE (");
    let lines: Vec<&str> = formatted.lines().collect();
    let columns: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim() == "COLUMNS")
        .collect();
    let emp_id_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("emp_id NUMBER PATH"))
        .collect();
    let emp_name_lines: Vec<&str> = formatted
        .lines()
        .filter(|line| line.trim_start().starts_with("emp_name VARCHAR2"))
        .collect();
    assert_eq!(columns.len(), 2, "{formatted}");
    assert_eq!(emp_id_lines.len(), 2, "{formatted}");
    assert_eq!(emp_name_lines.len(), 2, "{formatted}");

    let passing_idx = lines
        .iter()
        .position(|line| *line == passing)
        .expect("XMLTYPE PASSING line");
    let first_columns_idx = lines
        .iter()
        .position(|line| *line == columns[0])
        .expect("XMLTABLE COLUMNS line");
    let xmltype_close = lines[passing_idx + 1..first_columns_idx]
        .iter()
        .rev()
        .find(|line| line.trim() == ")")
        .expect("standalone XMLTYPE close");

    assert_eq!(xml_owner.trim(), "FROM XMLTABLE (");
    assert_eq!(passing.trim(), "PASSING XMLTYPE (");
    assert_eq!(json_owner.trim(), "FROM JSON_TABLE (");
    assert!(indent(passing) > indent(xml_owner), "{formatted}");
    assert_eq!(indent(xmltype_close), indent(passing), "{formatted}");
    assert!(indent(columns[0]) > indent(xml_owner), "{formatted}");
    assert!(indent(columns[1]) > indent(json_owner), "{formatted}");
    assert_eq!(
        indent(emp_id_lines[0]),
        indent(columns[0]) + 4,
        "{formatted}"
    );
    assert_eq!(
        indent(emp_name_lines[0]),
        indent(columns[0]) + 4,
        "{formatted}"
    );
    assert_eq!(
        indent(emp_id_lines[1]),
        indent(columns[1]) + 4,
        "{formatted}"
    );
    assert_eq!(
        indent(emp_name_lines[1]),
        indent(columns[1]) + 4,
        "{formatted}"
    );
    assert!(!formatted.contains("COLUMNS emp_id"), "{formatted}");

    let compact = format_stable(
        "SELECT x.id FROM XMLTABLE ('/r' PASSING payload COLUMNS id NUMBER PATH 'id') x;",
        DatabaseType::Oracle,
    );
    assert!(
        compact.contains("FROM XMLTABLE ('/r' PASSING payload COLUMNS id NUMBER PATH 'id') x;"),
        "{compact}"
    );

    let qualified = format_stable(
        r#"SELECT x.val
FROM XMLTABLE (
    t.passing
    PASSING XMLTYPE ('<r>
        <v>ok</v>
    </r>')
    COLUMNS val VARCHAR2(10) PATH 'v'
) x;

SELECT x.val
FROM XMLTABLE (
    '<r>
        <v>ok</v>
    </r>'
    PASSING t.columns
    COLUMNS val VARCHAR2(10) PATH 'v'
) x;

SELECT *
FROM XMLTABLE (
    '<r>
        <columns>ok</columns>
        <passing>ok</passing>
    </r>'
    PASSING :columns
    COLUMNS
        columns VARCHAR2(10) PATH 'columns',
        passing VARCHAR2(10) PATH 'passing'
) x;"#,
        DatabaseType::Oracle,
    );
    assert_eq!(
        line_starting_with(&qualified, "t.passing").trim(),
        "t.passing"
    );
    assert_eq!(
        line_starting_with(&qualified, "PASSING t.columns").trim(),
        "PASSING t.columns"
    );
    assert_eq!(
        line_starting_with(&qualified, "PASSING :columns").trim(),
        "PASSING :columns"
    );
    assert_eq!(
        line_starting_with(&qualified, "columns VARCHAR2").trim(),
        "columns VARCHAR2 (10) PATH 'columns',"
    );
    assert_eq!(
        line_starting_with(&qualified, "passing VARCHAR2").trim(),
        "passing VARCHAR2 (10) PATH 'passing'"
    );
    assert_eq!(
        qualified
            .lines()
            .filter(|line| line.trim() == "COLUMNS")
            .count(),
        3,
        "{qualified}"
    );

    let general_grouping = format_stable(
        "SELECT JSON_TABLE + ('a\nb') AS payload FROM dual;",
        DatabaseType::Oracle,
    );
    assert!(
        !general_grouping.contains("JSON_TABLE + (\n"),
        "{general_grouping}"
    );
}
