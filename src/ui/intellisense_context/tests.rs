use super::*;
use crate::ui::sql_editor::SqlEditorWidget;

fn tokenize(sql: &str) -> Vec<SqlToken> {
    SqlEditorWidget::tokenize_sql(sql)
}

/// Helper: tokenize SQL up to `|` marker (cursor position).
/// Returns (full_tokens, cursor_token_len).
fn split_at_cursor(sql: &str) -> (Vec<SqlToken>, usize) {
    use crate::ui::sql_editor::query_text::tokenize_sql_spanned;

    let cursor_pos = sql
        .find('|')
        .expect("SQL must contain '|' as cursor marker");
    let before = &sql[..cursor_pos];
    let after = &sql[cursor_pos + 1..];
    let full = format!("{}{}", before, after);
    let token_spans = tokenize_sql_spanned(&full);
    let cursor_token_len = token_spans.partition_point(|span| span.end <= cursor_pos);
    let full_tokens = token_spans.into_iter().map(|span| span.token).collect();
    (full_tokens, cursor_token_len)
}

fn analyze(sql: &str) -> CursorContext {
    let (full, cursor_token_len) = split_at_cursor(sql);
    analyze_cursor_context_owned(full, cursor_token_len)
}

fn table_names(ctx: &CursorContext) -> Vec<String> {
    ctx.tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect()
}

fn cte_names(ctx: &CursorContext) -> Vec<String> {
    ctx.ctes.iter().map(|c| c.name.to_uppercase()).collect()
}

fn has_name(names: &[String], wanted: &str) -> bool {
    names.iter().any(|name| name.eq_ignore_ascii_case(wanted))
}

// ─── Phase detection tests ───────────────────────────────────────────────

#[test]
fn phase_initial_empty() {
    let ctx = analyze("|");
    assert_eq!(ctx.phase, SqlPhase::Initial);
}

#[test]
fn phase_select_list() {
    let ctx = analyze("SELECT |");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_select_list_after_column() {
    let ctx = analyze("SELECT a, |");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn phase_select_list_inside_plsql_for_in_subquery() {
    let ctx = analyze(
        r#"CREATE OR REPLACE PACKAGE BODY oqt_demo_pkg AS
    PROCEDURE proc_fill_result_table (p_run_id IN NUMBER, p_min_sal IN NUMBER) IS
        v_row_no NUMBER := 0;
    BEGIN
        DELETE FROM oqt_tmp_result WHERE run_id = p_run_id;
        FOR r IN (
            SELECT emp_id,
                |,
                sal
            FROM oqt_emp
            WHERE sal >= p_min_sal
            ORDER BY sal
        ) LOOP
            v_row_no := v_row_no + 1;
        END LOOP;
    END;
END oqt_demo_pkg;"#,
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "OQT_EMP"),
        "expected oqt_emp in scope, got {:?}",
        names
    );
}

#[test]
fn phase_from_clause() {
    let ctx = analyze("SELECT a FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_where_clause() {
    let ctx = analyze("SELECT a FROM t WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn distinct_from_in_where_clause_does_not_trigger_from_clause() {
    let ctx = analyze("SELECT * FROM emp e WHERE e.sal IS DISTINCT FROM |");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP to remain in scope after IS DISTINCT FROM, got {:?}",
        names
    );
}

#[test]
fn not_distinct_from_in_where_clause_does_not_trigger_from_clause() {
    let ctx = analyze("SELECT * FROM emp e WHERE e.sal IS NOT DISTINCT FROM |");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP to remain in scope after IS NOT DISTINCT FROM, got {:?}",
        names
    );
}

#[test]
fn phase_join_on_clause() {
    let ctx = analyze("SELECT a FROM t1 JOIN t2 ON |");
    assert_eq!(ctx.phase, SqlPhase::JoinCondition);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_join_using_clause() {
    let ctx = analyze("SELECT * FROM employees e JOIN departments d USING (|)");
    assert_eq!(ctx.phase, SqlPhase::JoinUsingColumnList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(
        ctx.focused_tables,
        vec!["employees".to_string(), "departments".to_string()]
    );

    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn phase_join_using_clause_focuses_current_join_operands() {
    let ctx = analyze(
        "SELECT * FROM offices o JOIN employees e ON o.office_id = e.office_id \
         JOIN departments d USING (|)",
    );
    assert_eq!(ctx.phase, SqlPhase::JoinUsingColumnList);
    assert_eq!(
        ctx.focused_tables,
        vec!["employees".to_string(), "departments".to_string()]
    );
}

#[test]
fn phase_left_semi_join_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp e LEFT SEMI JOIN dept d ON e.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "SEMI"),
        "SEMI must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
}

#[test]
fn phase_left_anti_join_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp e LEFT ANTI JOIN dept d ON e.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "ANTI"),
        "ANTI must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
}

#[test]
fn phase_semi_join_without_left_modifier_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp SEMI JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "SEMI"),
        "SEMI must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_anti_join_without_left_modifier_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp ANTI JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "ANTI"),
        "ANTI must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_left_asof_join_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp e LEFT ASOF JOIN dept d ON e.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "ASOF"),
        "ASOF must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_left_join_after_as_does_not_capture_join_modifier_as_alias() {
    let ctx = analyze("SELECT * FROM emp AS LEFT JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "LEFT"),
        "LEFT must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_asof_join_without_left_modifier_keeps_join_modifier_out_of_aliases() {
    let ctx = analyze("SELECT * FROM emp ASOF JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "ASOF"),
        "ASOF must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_hash_join_hint_is_not_parsed_as_left_table_alias() {
    let ctx = analyze("SELECT * FROM emp HASH JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "HASH"),
        "HASH must remain a join hint keyword, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_loop_join_hint_is_not_parsed_as_left_table_alias() {
    let ctx = analyze("SELECT * FROM emp LOOP JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "LOOP"),
        "LOOP must remain a join hint keyword, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_merge_join_hint_is_not_parsed_as_left_table_alias() {
    let ctx = analyze("SELECT * FROM emp MERGE JOIN dept d ON emp.deptno = d.deptno WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "MERGE"),
        "MERGE must remain a join hint keyword, not a relation alias: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "right relation alias should remain visible: {:?}",
        aliases
    );
}

#[test]
fn phase_group_by() {
    let ctx = analyze("SELECT a FROM t GROUP BY |");
    assert_eq!(ctx.phase, SqlPhase::GroupByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_group_by_with_comment_between_keywords() {
    let ctx = analyze("SELECT a FROM t GROUP /*c*/ BY |");
    assert_eq!(ctx.phase, SqlPhase::GroupByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_having() {
    let ctx = analyze("SELECT a FROM t GROUP BY a HAVING |");
    assert_eq!(ctx.phase, SqlPhase::HavingClause);
    assert!(ctx.phase.is_column_context());
}
#[test]
fn within_group_order_by_does_not_switch_to_group_by_phase() {
    let ctx = analyze("SELECT LISTAGG(empno, ',') WITHIN GROUP (ORDER BY |) FROM emp");

    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "WITHIN GROUP ORDER BY should preserve FROM scope: {:?}",
        names
    );
}

#[test]
fn within_group_order_by_with_comment_does_not_switch_to_group_by_phase() {
    let ctx = analyze("SELECT LISTAGG(empno, ',') WITHIN /*x*/ GROUP /*y*/ (ORDER BY |) FROM emp");

    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "WITHIN GROUP ORDER BY with comments should preserve FROM scope: {:?}",
        names
    );
}

#[test]
fn phase_order_by() {
    let ctx = analyze("SELECT a FROM t ORDER BY |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_grant_with_grant_option_does_not_enter_with_clause() {
    let ctx = analyze("GRANT EXECUTE ON pkg_demo TO scott WITH | GRANT OPTION");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(
        ctx.ctes.is_empty(),
        "non-query WITH option must not create CTEs"
    );
}

#[test]
fn phase_query_with_read_only_option_leaves_query_context() {
    let ctx = analyze("CREATE VIEW v_emp AS SELECT * FROM emp WITH | READ ONLY");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(!ctx.phase.is_column_context());
    assert!(
        ctx.ctes.is_empty(),
        "query-tail WITH READ ONLY must not create CTEs"
    );
}

#[test]
fn phase_query_with_cascaded_check_option_leaves_query_context() {
    let ctx = analyze(
        "CREATE VIEW v_emp AS SELECT * FROM emp WHERE deptno > 10 WITH | CASCADED CHECK OPTION",
    );

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(!ctx.phase.is_column_context());
    assert!(
        ctx.ctes.is_empty(),
        "query-tail WITH CASCADED CHECK OPTION must not create CTEs"
    );
}

#[test]
fn phase_fetch_with_ties_leaves_query_context() {
    let ctx = analyze("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS WITH | TIES");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(!ctx.phase.is_column_context());
    assert!(
        ctx.ctes.is_empty(),
        "FETCH ... WITH TIES must not create CTEs"
    );
}

#[test]
fn phase_with_xmlnamespaces_clause_is_not_cte_column_context() {
    let ctx = analyze("WITH XMLNAMESPACES (DEFAULT | 'urn:emp') SELECT * FROM emp");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(!ctx.phase.is_column_context());
    assert!(
        ctx.ctes.is_empty(),
        "WITH XMLNAMESPACES must not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn phase_with_change_tracking_context_clause_is_not_cte_column_context() {
    let ctx = analyze("WITH CHANGE_TRACKING_CONTEXT (| 0x01) SELECT * FROM emp");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(!ctx.phase.is_column_context());
    assert!(
        ctx.ctes.is_empty(),
        "WITH CHANGE_TRACKING_CONTEXT must not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn phase_order_siblings_by() {
    let ctx = analyze("SELECT a FROM t ORDER SIBLINGS BY |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn straight_join_is_parsed_as_join_boundary() {
    let ctx = analyze("SELECT d.| FROM emp e STRAIGHT_JOIN dept d ON e.deptno = d.deptno");

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "DEPT"),
        "STRAIGHT_JOIN should expose right-side relation in scope: {:?}",
        names
    );
}

#[test]
fn straight_join_select_modifier_does_not_switch_to_from_clause() {
    let ctx = analyze("SELECT STRAIGHT_JOIN d.|, e.empno FROM dept d");

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "DEPT"),
        "SELECT modifier STRAIGHT_JOIN should not pollute relation scope: {:?}",
        names
    );
}

#[test]
fn phase_order_siblings_by_with_comment_between_keywords() {
    let ctx = analyze("SELECT a FROM t ORDER /*c1*/ SIBLINGS /*c2*/ BY |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_order_by_with_comment_between_keywords() {
    let ctx = analyze("SELECT a FROM t ORDER /*c*/ BY |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn unnest_function_argument_keeps_left_relation_visible() {
    let ctx = analyze("SELECT * FROM orders o, UNNEST(o.items) u WHERE o.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "left relation should remain visible after UNNEST: {:?}",
        names
    );
}

#[test]
fn unnest_relation_alias_is_collected_after_function_arguments() {
    let ctx = analyze("SELECT u.| FROM orders o, UNNEST(o.items) u WHERE o.id = 1");

    assert!(
        ctx.tables_in_scope.iter().any(
            |table| table.name.eq_ignore_ascii_case("u") && table.alias.as_deref() == Some("u")
        ),
        "UNNEST relation alias should be collected after function arguments: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn table_function_with_ordinality_keeps_alias_after_postfix_clause() {
    let ctx = analyze("SELECT u.| FROM orders o, UNNEST(o.items) WITH ORDINALITY u");

    assert!(
        ctx.tables_in_scope.iter().any(
            |table| table.name.eq_ignore_ascii_case("u") && table.alias.as_deref() == Some("u")
        ),
        "WITH ORDINALITY postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn table_function_with_offset_postfix_does_not_switch_to_pagination_phase() {
    let ctx = analyze("SELECT * FROM orders o, UNNEST(o.items) WITH OFFSET AS off WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "WITH OFFSET postfix should not reset relation scope: {:?}",
        names
    );
}

#[test]
fn table_function_with_offset_postfix_keeps_alias_after_postfix_clause() {
    let ctx = analyze("SELECT u.| FROM orders o, UNNEST(o.items) WITH OFFSET AS off u");

    assert!(
        ctx.tables_in_scope.iter().any(
            |table| table.name.eq_ignore_ascii_case("u") && table.alias.as_deref() == Some("u")
        ),
        "WITH OFFSET postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sqlite_indexed_by_postfix_keeps_alias_after_clause() {
    let ctx = analyze("SELECT o.| FROM orders INDEXED BY idx_orders_date o");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDERS")
                && table.alias.as_deref() == Some("o")),
        "INDEXED BY postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sqlite_not_indexed_postfix_keeps_alias_after_clause() {
    let ctx = analyze("SELECT o.| FROM orders NOT INDEXED o");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDERS")
                && table.alias.as_deref() == Some("o")),
        "NOT INDEXED postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn mysql_use_index_postfix_keeps_alias_after_clause() {
    let ctx = analyze("SELECT o.| FROM orders USE INDEX (idx_orders_date) o");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDERS")
                && table.alias.as_deref() == Some("o")),
        "USE INDEX postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn mysql_force_index_for_join_postfix_keeps_alias_after_clause() {
    let ctx = analyze(
        "SELECT o.| FROM orders FORCE INDEX FOR JOIN (idx_orders_date) o \
         JOIN order_items i ON i.order_id = o.id",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDERS")
                && table.alias.as_deref() == Some("o")),
        "FORCE INDEX FOR JOIN postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDER_ITEMS")
                && table.alias.as_deref() == Some("i")),
        "JOIN relation should still be parsed after FORCE INDEX clause: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn mysql_ignore_key_for_order_by_postfix_keeps_alias_after_clause() {
    let ctx = analyze("SELECT o.| FROM orders IGNORE KEY FOR ORDER BY (idx_orders_date) o");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("ORDERS")
                && table.alias.as_deref() == Some("o")),
        "IGNORE KEY FOR ORDER BY postfix should not block alias parsing: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn mysql_index_hint_without_alias_keeps_join_boundary_intact() {
    let ctx = analyze(
        "SELECT c.| FROM orders USE INDEX (idx_orders_date) \
         JOIN customers c ON c.id = orders.customer_id",
    );
    assert_eq!(ctx.phase, SqlPhase::SelectList);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "orders table should remain visible with USE INDEX hint: {:?}",
        names
    );
    assert!(
        names.contains(&"CUSTOMERS".to_string()),
        "JOIN target should remain visible after USE INDEX hint: {:?}",
        names
    );
}

#[test]
fn mysql_backtick_quoted_relation_and_alias_resolve_qualifier_tables() {
    let ctx = analyze("SELECT `o`.| FROM `sales schema`.`orders table` `o`");

    let resolved_alias = resolve_qualifier_tables("o", &ctx.tables_in_scope);
    assert_eq!(
        resolved_alias,
        vec!["sales schema.orders table".to_string()],
        "tables in scope: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved_quoted_alias = resolve_qualifier_tables("`o`", &ctx.tables_in_scope);
    assert_eq!(
        resolved_quoted_alias,
        vec!["sales schema.orders table".to_string()]
    );

    let resolved_quoted_name =
        resolve_qualifier_tables("`sales schema`.`orders table`", &ctx.tables_in_scope);
    assert_eq!(
        resolved_quoted_name,
        vec!["sales schema.orders table".to_string()]
    );
}

#[test]
fn mysql_backtick_quoted_identifier_with_embedded_dot_preserves_name() {
    let ctx = analyze("SELECT o.| FROM `sales.daily` o");

    let resolved_alias = resolve_qualifier_tables("o", &ctx.tables_in_scope);
    assert_eq!(resolved_alias, vec!["`sales.daily`".to_string()]);

    let resolved_name = resolve_qualifier_tables("`sales.daily`", &ctx.tables_in_scope);
    assert_eq!(resolved_name, vec!["`sales.daily`".to_string()]);
}

#[test]
fn unnest_argument_scope_can_resolve_left_relation_columns() {
    let ctx = analyze("SELECT * FROM orders o, UNNEST(o.|) u");

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "UNNEST argument scope should keep left relation visible: {:?}",
        names
    );
}

#[test]
fn table_collection_argument_scope_can_resolve_left_relation_columns() {
    let ctx = analyze("SELECT * FROM orders o, TABLE(o.|) t");

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "TABLE(...) argument scope should keep left relation visible: {:?}",
        names
    );
}

#[test]
fn phase_for_update_of_is_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["emp".to_string()]);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "FOR"),
        "FOR locking clause keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

#[test]
fn phase_for_share_of_is_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR SHARE OF |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_lock_in_share_mode_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp LOCK IN SHARE MODE |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP to remain visible after LOCK IN SHARE MODE, got {:?}",
        names
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "LOCK"),
        "LOCK clause keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

#[test]
fn phase_lock_in_share_mode_prefix_stays_post_query_context() {
    let ctx = analyze("SELECT * FROM emp LOCK |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "LOCK"),
        "incomplete LOCK IN SHARE MODE should not treat LOCK as alias: {:?}",
        aliases
    );
}

#[test]
fn lock_in_share_mode_keeps_existing_table_alias_resolution() {
    let ctx = analyze("SELECT e.| FROM emp e LOCK IN SHARE MODE");
    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["emp".to_string()]);
}

#[test]
fn phase_for_no_key_update_of_is_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR NO KEY UPDATE OF |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_key_share_of_is_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR KEY SHARE OF |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_wait_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE WAIT |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_nowait_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE NOWAIT |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_skip_locked_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE SKIP LOCKED |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_of_wait_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno WAIT |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_of_nowait_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno NOWAIT |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_of_skip_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno SKIP |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_of_identifier_named_skip_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF skip |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_qualified_identifier_named_skip_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp e FOR UPDATE OF e.skip |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_qualified_identifier_named_wait_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp e FOR UPDATE OF e.wait |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_qualified_identifier_named_nowait_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp e FOR UPDATE OF e.nowait |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_additional_identifier_named_skip_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno, skip |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_identifier_then_skip_transitions_to_non_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno skip |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_update_of_skip_locked_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR UPDATE OF empno SKIP LOCKED |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_share_of_skip_is_not_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR SHARE OF empno SKIP |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn phase_for_share_of_identifier_named_skip_stays_column_context() {
    let ctx = analyze("SELECT * FROM emp FOR SHARE OF skip |");
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_for_update_of_focuses_current_query_tables() {
    let ctx = analyze(
        "SELECT * FROM parent p WHERE EXISTS (SELECT 1 FROM child c WHERE c.parent_id = p.id FOR UPDATE OF |)",
    );
    assert_eq!(ctx.phase, SqlPhase::LockingColumnList);
    assert_eq!(ctx.focused_tables, vec!["child".to_string()]);
}

#[test]
fn phase_for_read_only_clause_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp FOR READ ONLY |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases
            .iter()
            .all(|alias| alias != "FOR" && alias != "READ"),
        "FOR READ ONLY keywords must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_read_write_clause_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp FOR READ WRITE |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases
            .iter()
            .all(|alias| alias != "FOR" && alias != "READ" && alias != "WRITE"),
        "FOR READ WRITE keywords must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_json_clause_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp FOR JSON PATH, ROOT('employees') |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "JSON"),
        "FOR JSON keywords must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_json_clause_after_where_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp WHERE deptno = 10 FOR JSON PATH |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "JSON"),
        "FOR JSON after WHERE must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_xml_clause_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp FOR XML PATH('employee'), TYPE |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "XML"),
        "FOR XML keywords must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_xml_clause_after_where_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp WHERE deptno = 10 FOR XML PATH('employee') |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "XML"),
        "FOR XML after WHERE must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_for_browse_clause_after_where_is_not_table_context() {
    let ctx = analyze("SELECT * FROM emp WHERE deptno = 10 FOR BROWSE |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "BROWSE"),
        "FOR BROWSE after WHERE must not be parsed as relation aliases: {:?}",
        aliases
    );
}

#[test]
fn phase_window_clause_is_column_context() {
    let ctx = analyze("SELECT a FROM t WINDOW w AS (PARTITION BY |)");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_window_keyword_is_column_context() {
    let ctx = analyze("SELECT a FROM t WINDOW |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_qualify_clause_is_column_context() {
    let ctx = analyze("SELECT a FROM t QUALIFY |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_limit_clause_is_not_table_context() {
    let ctx = analyze("SELECT a FROM t LIMIT |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(names.contains(&"T".to_string()), "tables: {:?}", names);
}

#[test]
fn fetch_first_clause_keyword_is_not_parsed_as_relation_alias() {
    let ctx = analyze("SELECT * FROM emp FETCH FIRST | ROWS ONLY");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "FETCH"),
        "FETCH pagination keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

#[test]
fn phase_offset_clause_is_not_table_context() {
    let ctx = analyze("SELECT a FROM t OFFSET |");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(names.contains(&"T".to_string()), "tables: {:?}", names);
}

#[test]
fn phase_fetch_clause_is_not_table_context() {
    let ctx = analyze("SELECT a FROM t FETCH FIRST | ROWS ONLY");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(names.contains(&"T".to_string()), "tables: {:?}", names);
}

#[test]
fn phase_update_set() {
    let ctx = analyze("UPDATE t SET |");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn phase_mysql_on_duplicate_key_update_is_column_context() {
    let ctx = analyze("INSERT INTO t (id, val) VALUES (1, 2) ON DUPLICATE KEY UPDATE |");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert!(ctx.phase.is_column_context());
    assert!(!ctx.phase.is_table_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn phase_postgres_on_conflict_do_update_is_column_context() {
    let ctx = analyze("INSERT INTO t (id, val) VALUES (1, 2) ON CONFLICT (id) DO UPDATE SET |");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert!(ctx.phase.is_column_context());
    assert!(!ctx.phase.is_table_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn phase_postgres_on_conflict_target_list_prefers_insert_target() {
    let ctx = analyze(
        "INSERT INTO t (id, val) VALUES (1, 2) ON CONFLICT (|) DO UPDATE SET val = EXCLUDED.val",
    );
    assert_eq!(ctx.phase, SqlPhase::ConflictTargetList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn postgres_excluded_qualifier_resolves_to_insert_target() {
    let ctx = analyze(
        "INSERT INTO t (id, val) VALUES (1, 2) ON CONFLICT (id) DO UPDATE SET val = EXCLUDED.|",
    );
    let tables = resolve_qualifier_tables("EXCLUDED", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["t".to_string()]);
}

#[test]
fn phase_insert_into() {
    let ctx = analyze("INSERT INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_insert_overwrite_table_is_table_context() {
    let ctx = analyze("INSERT OVERWRITE TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_insert_overwrite_directory_is_not_table_context() {
    let ctx = analyze("INSERT OVERWRITE DIRECTORY |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_insert_into_table_is_table_context() {
    let ctx = analyze("INSERT INTO TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_replace_without_into_is_table_context() {
    let ctx = analyze("REPLACE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_replace_into_is_table_context() {
    let ctx = analyze("REPLACE INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_replace_low_priority_into_is_table_context() {
    let ctx = analyze("REPLACE LOW_PRIORITY INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_replace_low_priority_without_into_is_table_context() {
    let ctx = analyze("REPLACE LOW_PRIORITY |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_insert_ignore_without_into_is_table_context() {
    let ctx = analyze("INSERT IGNORE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_replace_into_column_list_is_column_context() {
    let ctx = analyze("REPLACE INTO t (|) VALUES (1)");
    assert_eq!(ctx.phase, SqlPhase::InsertColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn replace_low_priority_modifier_is_not_captured_as_target_table() {
    let ctx = analyze("REPLACE LOW_PRIORITY INTO audit_emp (|) VALUES (1)");
    assert_eq!(ctx.phase, SqlPhase::InsertColumnList);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "AUDIT_EMP"),
        "expected REPLACE target table in scope, got {:?}",
        names
    );
    assert!(
        names.iter().all(|name| name != "LOW_PRIORITY"),
        "modifier keyword must not be parsed as target table name: {:?}",
        names
    );
}

#[test]
fn insert_low_priority_ignore_without_into_on_duplicate_update_is_column_context() {
    let ctx =
        analyze("INSERT LOW_PRIORITY IGNORE audit_emp (id) VALUES (1) ON DUPLICATE KEY UPDATE |");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["audit_emp".to_string()]);
}

#[test]
fn replace_function_call_in_select_list_stays_column_context() {
    let ctx = analyze("SELECT REPLACE(|, 'a', 'b') FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn create_or_replace_view_select_list_is_column_context() {
    // `CREATE OR REPLACE VIEW` — REPLACE is a DDL modifier, not a DML statement.
    // The SELECT list inside the view body must be a column context with the
    // view's row sources visible.
    let ctx = analyze("CREATE OR REPLACE VIEW v_emp AS SELECT e.| FROM employees e");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
    assert!(!ctx.phase.is_table_context());
    let table_names: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .map(|t| {
            t.alias
                .clone()
                .unwrap_or_else(|| t.name.clone())
                .to_uppercase()
        })
        .collect();
    assert!(
        table_names.iter().any(|n| n == "E"),
        "alias `e` for employees must be in scope inside CREATE OR REPLACE VIEW body: {table_names:?}"
    );
}

#[test]
fn create_or_replace_view_from_clause_is_table_context() {
    let ctx = analyze("CREATE OR REPLACE VIEW v_emp AS SELECT e.emp_code FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn create_or_replace_view_where_clause_is_column_context() {
    let ctx =
        analyze("CREATE OR REPLACE VIEW v_emp AS SELECT e.emp_code FROM employees e WHERE e.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
    let table_names: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .map(|t| {
            t.alias
                .clone()
                .unwrap_or_else(|| t.name.clone())
                .to_uppercase()
        })
        .collect();
    assert!(
        table_names.iter().any(|n| n == "E"),
        "alias `e` must be in scope in CREATE OR REPLACE VIEW WHERE clause: {table_names:?}"
    );
}

#[test]
fn create_or_replace_view_does_not_register_view_keyword_as_table() {
    // After the fix, `VIEW` must not be collected as a DML target table.
    // Cursor is at the WHERE clause so all tables from the FROM clause are visible.
    let ctx =
        analyze("CREATE OR REPLACE VIEW v_emp AS SELECT e.emp_code FROM employees e WHERE e.|");
    let raw_names: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        !raw_names.iter().any(|n| n == "VIEW"),
        "`VIEW` must not be registered as a relation name: {raw_names:?}"
    );
    assert!(
        raw_names.iter().any(|n| n == "EMPLOYEES"),
        "`employees` must be registered as a relation: {raw_names:?}"
    );
}

#[test]
fn create_or_replace_view_statement_kind_is_not_insert() {
    // The statement_kind must NOT be Insert after `CREATE OR REPLACE VIEW ... AS SELECT`.
    // Before the fix, REPLACE was treated as DML INSERT which carried Insert kind
    // into the SELECT body via preserve_insert_statement_kind.
    let ctx =
        analyze("CREATE OR REPLACE VIEW v_emp AS SELECT e.emp_code FROM employees e WHERE e.|");
    // WhereClause is a column context; focused table should be "employees", not "VIEW".
    assert!(ctx.phase.is_column_context());
    let focused: Vec<String> = ctx
        .focused_tables
        .iter()
        .map(|n| n.to_uppercase())
        .collect();
    assert!(
        !focused.iter().any(|n| n == "VIEW"),
        "`VIEW` must not appear in focused_tables: {focused:?}"
    );
}

#[test]
fn phase_truncate_table_is_table_context() {
    let ctx = analyze("TRUNCATE TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_truncate_table_with_comment_is_table_context() {
    let ctx = analyze("TRUNCATE /* ddl */ TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_lock_table_is_table_context() {
    let ctx = analyze("LOCK TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_lock_table_with_comment_is_table_context() {
    let ctx = analyze("LOCK /* ddl */ TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_mysql_lock_tables_is_table_context() {
    let ctx = analyze("LOCK TABLES |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_mysql_lock_tables_with_comment_is_table_context() {
    let ctx = analyze("LOCK /* mariadb */ TABLES |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn mysql_lock_tables_mode_keyword_is_not_parsed_as_alias() {
    let ctx = analyze("LOCK TABLES emp READ |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("READ")),
        "LOCK TABLES READ mode keyword must not be parsed as alias: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn mysql_lock_tables_with_comment_mode_keyword_is_not_parsed_as_alias() {
    let ctx = analyze("LOCK /* mariadb */ TABLES emp READ |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("READ")),
        "LOCK TABLES READ mode keyword must not be parsed as alias when LOCK and TABLES are comment-separated: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn mysql_lock_tables_comma_reopens_table_target_context() {
    let ctx = analyze("LOCK TABLES emp READ, |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn mysql_lock_tables_with_comment_comma_reopens_table_target_context() {
    let ctx = analyze("LOCK /* mariadb */ TABLES emp READ, |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn mysql_lock_tables_quoted_read_alias_is_preserved() {
    let ctx = analyze(r"LOCK TABLES emp AS `read` READ, |");

    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(
        ctx.tables_in_scope.iter().any(|table| {
            table
                .alias
                .as_deref()
                .is_some_and(|alias| alias.eq_ignore_ascii_case("read"))
        }),
        "quoted alias named READ should be preserved in LOCK TABLES: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn phase_lock_table_in_clause_is_not_table_context() {
    let ctx = analyze("LOCK TABLE emp IN |");
    assert_ne!(ctx.phase, SqlPhase::IntoClause);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_lock_table_share_update_mode_does_not_switch_to_update_target() {
    let ctx = analyze("LOCK TABLE emp IN SHARE UPDATE |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_lock_table_row_share_update_mode_does_not_switch_to_update_target() {
    let ctx = analyze("LOCK TABLE emp IN ROW SHARE UPDATE |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_drop_table_is_table_context() {
    let ctx = analyze("DROP TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_alter_table_is_table_context() {
    let ctx = analyze("ALTER TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_flashback_table_is_table_context() {
    let ctx = analyze("FLASHBACK TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_rename_table_is_table_context() {
    let ctx = analyze("RENAME TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_oracle_rename_target_is_table_context() {
    let ctx = analyze("RENAME |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn oracle_rename_to_clause_is_not_treated_as_table_alias() {
    let ctx = analyze("RENAME employees TO |");

    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("TO")),
        "TO keyword in RENAME syntax must not be parsed as relation alias: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn phase_analyze_table_is_table_context() {
    let ctx = analyze("ANALYZE TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_optimize_table_is_table_context() {
    let ctx = analyze("OPTIMIZE TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_check_table_is_table_context() {
    let ctx = analyze("CHECK TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_repair_table_is_table_context() {
    let ctx = analyze("REPAIR TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_table_is_table_context() {
    let ctx = analyze("COMMENT ON TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_table_with_inline_comment_is_table_context() {
    let ctx = analyze("COMMENT ON /* inline */ TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_view_is_table_context() {
    let ctx = analyze("COMMENT ON VIEW |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_materialized_view_is_table_context() {
    let ctx = analyze("COMMENT ON MATERIALIZED VIEW |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_editioning_view_is_table_context() {
    let ctx = analyze("COMMENT ON EDITIONING VIEW |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_editioning_view_with_inline_comment_is_table_context() {
    let ctx = analyze("COMMENT ON EDITIONING /* inline */ VIEW |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_column_is_table_context() {
    let ctx = analyze("COMMENT ON COLUMN |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_comment_on_column_with_inline_comment_is_table_context() {
    let ctx = analyze("COMMENT ON /* inline */ COLUMN |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_index_on_is_table_context() {
    let ctx = analyze("CREATE INDEX idx_emp_dept ON |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_trigger_on_is_table_context() {
    let ctx = analyze("CREATE TRIGGER trg_emp_audit ON |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_table_is_table_context() {
    let ctx = analyze("CREATE TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_global_temporary_table_is_table_context() {
    let ctx = analyze("CREATE GLOBAL TEMPORARY TABLE |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_table_as_select_is_table_context_before_name() {
    let ctx = analyze("CREATE TABLE | AS SELECT 1");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_create_table_definition_keyword_is_not_table_context() {
    let ctx = analyze("CREATE TABLE demo (id BI|)");
    assert!(
        !ctx.phase.is_table_context(),
        "CREATE TABLE definition should not stay in table-target context: {:?}",
        ctx.phase
    );
}

#[test]
fn phase_create_table_option_keyword_is_not_table_context() {
    let ctx = analyze("CREATE TABLE demo (id INT) ENG|");
    assert!(
        !ctx.phase.is_table_context(),
        "CREATE TABLE option list should not stay in table-target context: {:?}",
        ctx.phase
    );
}

#[test]
fn phase_create_table_references_target_is_table_context() {
    let ctx = analyze(
        "CREATE TABLE demo (dept_id INT, CONSTRAINT fk_demo FOREIGN KEY (dept_id) REFERENCES |)",
    );
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_join_on_remains_join_condition_context() {
    let ctx = analyze("SELECT * FROM emp e JOIN dept d ON |");
    assert_eq!(ctx.phase, SqlPhase::JoinCondition);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_values() {
    let ctx = analyze("INSERT INTO t (a) VALUES |");
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn values_clause_is_column_context_for_expression_completion() {
    let ctx = analyze("INSERT INTO t (a) VALUES (|");
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn pivot_clause_is_column_context_for_aggregate_and_for_expression() {
    let ctx = analyze("SELECT * FROM src PIVOT (SUM(|) AS sum_sal FOR deptno IN (10 AS d10))");
    assert_eq!(ctx.phase, SqlPhase::PivotClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_insert_returning_is_column_context() {
    let ctx = analyze("INSERT INTO t (a) VALUES (1) RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_update_returning_is_column_context() {
    let ctx = analyze("UPDATE t SET a = 1 RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_delete_returning_is_column_context() {
    let ctx = analyze("DELETE FROM t WHERE a = 1 RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_insert_returning_into_does_not_switch_to_table_context() {
    let ctx = analyze("INSERT INTO t (a) VALUES (1) RETURNING a INTO |");
    assert_eq!(ctx.phase, SqlPhase::ReturningIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_update_returning_into_does_not_switch_to_table_context() {
    let ctx = analyze("UPDATE t SET a = 1 RETURNING a INTO |");
    assert_eq!(ctx.phase, SqlPhase::ReturningIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_delete_returning_into_does_not_switch_to_table_context() {
    let ctx = analyze("DELETE FROM t WHERE a = 1 RETURNING a INTO |");
    assert_eq!(ctx.phase, SqlPhase::ReturningIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_delete_returning_bulk_collect_into_does_not_switch_to_table_context() {
    let ctx = analyze("DELETE FROM t WHERE a = 1 RETURNING a BULK COLLECT INTO |");
    assert_eq!(ctx.phase, SqlPhase::ReturningIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_select_into_is_not_table_context() {
    let ctx = analyze("SELECT deptno INTO | FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_merge_returning_into_is_not_table_context() {
    let ctx = analyze(
        "MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val RETURNING t.id INTO |",
    );
    assert_eq!(ctx.phase, SqlPhase::ReturningIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_fetch_into_is_variable_context() {
    let ctx = analyze("BEGIN FETCH c INTO |; END;");
    assert_eq!(ctx.phase, SqlPhase::FetchIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_execute_immediate_into_is_variable_context() {
    let ctx = analyze("BEGIN EXECUTE IMMEDIATE 'select count(*) from emp' INTO |; END;");
    assert_eq!(ctx.phase, SqlPhase::ExecuteIntoTarget);
    assert!(ctx.phase.is_variable_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_execute_immediate_using_is_bind_context() {
    let ctx = analyze(
        "BEGIN EXECUTE IMMEDIATE 'select count(*) from emp where deptno = :1' INTO l_cnt USING |; END;",
    );
    assert_eq!(ctx.phase, SqlPhase::UsingBindList);
    assert!(ctx.phase.is_bind_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_open_for_select_using_is_bind_context() {
    let ctx = analyze("BEGIN OPEN c FOR SELECT empno FROM emp WHERE deptno = :1 USING |; END;");
    assert_eq!(ctx.phase, SqlPhase::UsingBindList);
    assert!(ctx.phase.is_bind_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_open_for_dynamic_sql_using_is_bind_context() {
    let ctx = analyze("BEGIN OPEN c FOR l_sql USING |; END;");
    assert_eq!(ctx.phase, SqlPhase::UsingBindList);
    assert!(ctx.phase.is_bind_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_insert_log_errors_into_is_table_context() {
    let ctx = analyze("INSERT INTO employees(id) VALUES (1) LOG ERRORS INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_update_log_errors_into_is_table_context() {
    let ctx = analyze("UPDATE employees SET salary = salary + 1 LOG ERRORS INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_delete_log_errors_into_is_table_context() {
    let ctx = analyze("DELETE FROM employees WHERE salary > 0 LOG ERRORS INTO |");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_merge_log_errors_into_is_table_context() {
    let ctx = analyze(
        "MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val LOG ERRORS INTO |",
    );
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_merge_log_errors_reject_limit_clause_is_not_table_context() {
    let ctx = analyze(
        "MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val LOG ERRORS INTO err$_target REJECT | LIMIT UNLIMITED",
    );
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn phase_truncate_table_reuse_storage_clause_is_not_table_context() {
    let ctx = analyze("TRUNCATE TABLE employees REUSE STORAGE |");
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn merge_log_errors_into_does_not_capture_reject_keyword_as_alias() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET t.val = s.val \
         LOG ERRORS INTO err$_target REJECT LIMIT UNLIMITED WHERE |",
    );

    let err_table = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("err$_target"));

    assert!(err_table.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        err_table
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("REJECT")),
        "LOG ERRORS INTO table alias must not be parsed as REJECT: {:?}",
        err_table
    );
}

#[test]
fn insert_log_errors_into_does_not_capture_reject_keyword_as_alias() {
    let ctx = analyze(
        "INSERT INTO target_table (id) VALUES (1) \
         LOG ERRORS INTO err$_target REJECT LIMIT UNLIMITED RETURNING |",
    );

    let err_table = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("err$_target"));

    assert!(err_table.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        err_table
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("REJECT")),
        "LOG ERRORS INTO table alias must not be parsed as REJECT: {:?}",
        err_table
    );
}

#[test]
fn update_log_errors_into_does_not_capture_reject_keyword_as_alias() {
    let ctx = analyze(
        "UPDATE target_table SET val = 1 \
         LOG ERRORS INTO err$_target REJECT LIMIT UNLIMITED RETURNING |",
    );

    let err_table = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("err$_target"));

    assert!(err_table.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        err_table
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("REJECT")),
        "LOG ERRORS INTO table alias must not be parsed as REJECT: {:?}",
        err_table
    );
}

#[test]
fn delete_log_errors_into_does_not_capture_reject_keyword_as_alias() {
    let ctx = analyze(
        "DELETE FROM target_table WHERE id > 0 \
         LOG ERRORS INTO err$_target REJECT LIMIT UNLIMITED RETURNING |",
    );

    let err_table = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("err$_target"));

    assert!(err_table.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        err_table
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("REJECT")),
        "LOG ERRORS INTO table alias must not be parsed as REJECT: {:?}",
        err_table
    );
}

#[test]
fn phase_select_json_value_returning_clause_stays_column_context() {
    let ctx = analyze("SELECT JSON_VALUE(payload, '$.id' RETURNING | NUMBER) FROM events e");

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EVENTS"),
        "expected EVENTS table in scope, got {:?}",
        names
    );
}

#[test]
fn phase_select_json_query_with_wrapper_stays_column_context() {
    let ctx = analyze("SELECT JSON_QUERY(e.payload, '$.items[*]' WITH | WRAPPER) FROM events e");

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EVENTS"),
        "expected EVENTS table in scope inside JSON_QUERY WITH WRAPPER, got {:?}",
        names
    );
}

#[test]
fn phase_select_json_object_with_unique_keys_stays_column_context() {
    let ctx = analyze(
        "SELECT JSON_OBJECT('id' VALUE e.empno, 'name' VALUE e.ename WITH | UNIQUE KEYS) FROM emp e",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP table in scope inside JSON_OBJECT WITH UNIQUE KEYS, got {:?}",
        names
    );
}

#[test]
fn phase_select_json_object_with_unique_keys_after_comment_stays_column_context() {
    let ctx = analyze(
        "SELECT JSON_OBJECT('id' VALUE e.empno, 'name' VALUE e.ename /*opts*/ WITH | UNIQUE KEYS) FROM emp e",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP table in scope inside commented JSON_OBJECT WITH UNIQUE KEYS, got {:?}",
        names
    );
}

#[test]
fn phase_select_json_transform_set_stays_column_context() {
    let ctx = analyze(
        "SELECT JSON_TRANSFORM(e.payload, SET | '$.status' = 'DONE') AS payload2 FROM emp_json e",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP_JSON"),
        "expected EMP_JSON table in scope inside JSON_TRANSFORM SET, got {:?}",
        names
    );
}

#[test]
fn phase_select_json_transform_set_after_comment_stays_column_context() {
    let ctx = analyze(
        "SELECT JSON_TRANSFORM(e.payload, /*op*/ SET | '$.status' = 'DONE') AS payload2 FROM emp_json e",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP_JSON"),
        "expected EMP_JSON table in scope inside commented JSON_TRANSFORM SET, got {:?}",
        names
    );
}

#[test]
fn phase_where_json_query_returning_clause_stays_where_context() {
    let ctx = analyze("SELECT * FROM events e WHERE JSON_QUERY(e.payload, '$' RETURNING | VARCHAR2(4000)) IS NOT NULL");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EVENTS"),
        "expected EVENTS table in scope, got {:?}",
        names
    );
}

#[test]
fn only_without_parentheses_keeps_underlying_table_name_and_alias() {
    let ctx = analyze("SELECT e.| FROM ONLY employees e WHERE e.id > 0");

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMPLOYEES"),
        "ONLY relation wrapper should preserve underlying table name: {:?}",
        names
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().any(|alias| alias == "E"),
        "ONLY relation wrapper alias should be captured: {:?}",
        aliases
    );

    assert!(
        aliases.iter().all(|alias| alias != "ONLY"),
        "ONLY keyword must not be parsed as alias: {:?}",
        aliases
    );
}

#[test]
fn phase_connect_by() {
    let ctx = analyze("SELECT a FROM t START WITH a = 1 CONNECT BY |");
    assert_eq!(ctx.phase, SqlPhase::ConnectByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_connect_by_with_comment_between_keywords() {
    let ctx = analyze("SELECT a FROM t START WITH a = 1 CONNECT /*c*/ BY |");
    assert_eq!(ctx.phase, SqlPhase::ConnectByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_start_with() {
    let ctx = analyze("SELECT a FROM t START WITH |");
    assert_eq!(ctx.phase, SqlPhase::StartWithClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_start_with_with_comment_between_keywords() {
    let ctx = analyze("SELECT a FROM t START /*c*/ WITH |");
    assert_eq!(ctx.phase, SqlPhase::StartWithClause);
    assert!(ctx.phase.is_column_context());
}

// ─── Depth tracking tests ────────────────────────────────────────────────

#[test]
fn depth_zero_at_top_level() {
    let ctx = analyze("SELECT | FROM t");
    assert_eq!(ctx.depth, 0);
}

#[test]
fn depth_one_in_subquery() {
    let ctx = analyze("SELECT * FROM (SELECT |");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn depth_two_in_nested_subquery() {
    let ctx = analyze("SELECT * FROM (SELECT * FROM (SELECT |");
    assert_eq!(ctx.depth, 2);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn depth_returns_to_zero_after_subquery() {
    let ctx = analyze("SELECT * FROM (SELECT 1 FROM dual) WHERE |");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn depth_in_subquery_where_clause() {
    let ctx = analyze("SELECT * FROM (SELECT a FROM t WHERE |");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn depth_in_subquery_from_clause() {
    let ctx = analyze("SELECT * FROM (SELECT a FROM |");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::FromClause);
}

// ─── Table collection tests ──────────────────────────────────────────────

#[test]
fn collect_single_table() {
    let ctx = analyze("SELECT | FROM employees");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn collect_multiple_tables() {
    let ctx = analyze("SELECT | FROM employees e, departments d");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn collect_join_tables() {
    let ctx = analyze("SELECT | FROM employees e JOIN departments d ON e.dept_id = d.id");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn collect_table_with_schema_prefix() {
    let ctx = analyze("SELECT | FROM hr.employees");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn collect_table_with_dblink_suffix() {
    let ctx = analyze("SELECT | FROM hr.employees@prod_link");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.EMPLOYEES@PROD_LINK".to_string()),
        "dblink-qualified relation should be preserved as a single table reference: {:?}",
        names
    );
}

#[test]
fn collect_table_with_dotted_dblink_suffix() {
    let ctx = analyze("SELECT | FROM employees@hq.prod_link e");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES@HQ.PROD_LINK".to_string()),
        "dotted dblink-qualified relation should be preserved as a single table reference: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias after dblink-qualified relation should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn collect_quoted_table_and_alias() {
    let ctx = analyze(r#"SELECT "e".| FROM "Emp Table" "e""#);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMP TABLE".to_string()),
        "quoted table should be normalized into scope: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|t| t.alias.as_deref() == Some("e")),
        "quoted alias should be normalized into scope: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn collect_quoted_table_with_dot_keeps_lookup_safe_form() {
    let ctx = analyze(r#"SELECT | FROM "A.B" t"#);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"\"A.B\"".to_string()),
        "quoted dotted table should preserve quoted form to avoid schema fallback ambiguity: {:?}",
        names
    );
}

#[test]
fn collect_table_ignores_numeric_starting_token() {
    let ctx = analyze("SELECT | FROM 123abc");
    let names = table_names(&ctx);
    assert!(
        !names.iter().any(|name| name == "123ABC"),
        "numeric-leading token should not be treated as table identifier: {:?}",
        names
    );
}

#[test]
fn extract_select_list_columns_ignores_numeric_literals() {
    let tokens = tokenize("SELECT 1e3, emp1, 42 FROM dual");
    let columns = extract_select_list_columns(&tokens);
    assert!(columns.iter().any(|name| name.eq_ignore_ascii_case("emp1")));
    assert!(!columns.iter().any(|name| name.eq_ignore_ascii_case("1e3")));
    assert!(!columns.iter().any(|name| name == "42"));
}

#[test]
fn collect_multiple_joins() {
    let ctx = analyze(
        "SELECT | FROM employees e \
         JOIN departments d ON e.dept_id = d.id \
         LEFT JOIN locations l ON d.loc_id = l.id",
    );
    let names = table_names(&ctx);
    assert!(names.contains(&"EMPLOYEES".to_string()));
    assert!(names.contains(&"DEPARTMENTS".to_string()));
    assert!(names.contains(&"LOCATIONS".to_string()));
}

#[test]
fn collect_table_aliases() {
    let ctx = analyze("SELECT | FROM employees e");
    assert!(ctx
        .tables_in_scope
        .iter()
        .any(|t| t.alias.as_deref() == Some("e")));
}

#[test]
fn collect_table_as_alias() {
    let ctx = analyze("SELECT | FROM employees AS emp");
    assert!(ctx
        .tables_in_scope
        .iter()
        .any(|t| t.alias.as_deref() == Some("emp")));
}

#[test]
fn collect_table_alias_with_inline_comment_after_table_name() {
    let ctx = analyze("SELECT e.| FROM employees /* alias marker */ e");

    let employee_scope = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("employees"));
    assert!(
        employee_scope.is_some(),
        "tables: {:?}",
        ctx.tables_in_scope
    );

    assert_eq!(
        employee_scope.and_then(|table| table.alias.as_deref()),
        Some("e")
    );
}

#[test]
fn collect_subquery_alias_with_comment_after_as_keyword() {
    let ctx = analyze("SELECT sq.| FROM (SELECT 1 AS id FROM dual) AS /* alias marker */ sq");

    let subquery_scope = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("sq"));
    assert!(
        subquery_scope.is_some(),
        "tables: {:?}",
        ctx.tables_in_scope
    );

    assert_eq!(
        subquery_scope.and_then(|table| table.alias.as_deref()),
        Some("sq")
    );
}

// ─── Subquery alias tests ────────────────────────────────────────────────

#[test]
fn subquery_alias_in_from() {
    let ctx = analyze("SELECT u.| FROM (SELECT id, name FROM users) u");
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("u")),
        "subquery alias 'u' should be in scope: {:?}",
        names
    );
}

#[test]
fn subquery_alias_with_as() {
    let ctx = analyze("SELECT sub.| FROM (SELECT id FROM t) AS sub");
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("sub")),
        "subquery alias 'sub' should be in scope: {:?}",
        names
    );
}

#[test]
fn subquery_alias_with_column_list_is_recognized() {
    let ctx = analyze("SELECT * FROM (SELECT 1 AS n FROM dual) sub(n) WHERE |");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"SUB".to_string()),
        "subquery alias with column list should be in scope: {:?}",
        names
    );
}

#[test]
fn subquery_alias_with_as_and_column_list_is_recognized() {
    let ctx = analyze("SELECT * FROM (SELECT 1 AS n FROM dual) AS sub(n) WHERE |");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"SUB".to_string()),
        "AS subquery alias with column list should be in scope: {:?}",
        names
    );
}

#[test]
fn subquery_alias_mixed_with_table() {
    let ctx = analyze("SELECT | FROM users u, (SELECT id FROM orders) o");
    let names = table_names(&ctx);
    assert!(names.contains(&"USERS".to_string()));
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("o")),
        "subquery alias 'o' should be in scope: {:?}",
        names
    );
}

// ─── CTE (WITH clause) tests ────────────────────────────────────────────

#[test]
fn cte_simple() {
    let ctx = analyze("WITH cte AS (SELECT 1 AS n FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(cte_n.contains(&"CTE".to_string()), "CTEs: {:?}", cte_n);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("cte")),
        "CTE should be in table scope: {:?}",
        names
    );
}

#[test]
fn cte_multiple() {
    let ctx =
        analyze("WITH a AS (SELECT 1 FROM dual), b AS (SELECT 2 FROM dual) SELECT | FROM a, b");
    let cte_n = cte_names(&ctx);
    assert!(cte_n.contains(&"A".to_string()), "CTEs: {:?}", cte_n);
    assert!(cte_n.contains(&"B".to_string()), "CTEs: {:?}", cte_n);
}

#[test]
fn cte_with_explicit_columns() {
    let ctx = analyze("WITH cte(x, y) AS (SELECT 1, 2 FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(cte_n.contains(&"CTE".to_string()));
    let cte_def = ctx
        .ctes
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("cte"))
        .unwrap();
    assert_eq!(cte_def.explicit_columns.len(), 2);
}

#[test]
fn cte_explicit_column_list_is_column_context() {
    let ctx = analyze("WITH cte(x, |) AS (SELECT 1, 2 FROM dual) SELECT * FROM cte");
    assert_eq!(ctx.phase, SqlPhase::CteColumnList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["cte".to_string()]);
}

#[test]
fn cte_cursor_in_main_query_where() {
    let ctx = analyze("WITH temp AS (SELECT id, name FROM users) SELECT * FROM temp WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert_eq!(ctx.depth, 0);
    let names = table_names(&ctx);
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("temp")));
}

#[test]
fn insert_with_cte_source_query_keeps_cte_visible() {
    let ctx =
        analyze("INSERT INTO audit_log WITH recent AS (SELECT 1 AS id FROM dual) SELECT recent.| FROM recent");

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "RECENT"),
        "insert-source WITH must keep CTE visible: {:?}",
        cte_names(&ctx)
    );
    let recent_cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("recent"))
        .expect("expected recent CTE in insert source query");
    assert_eq!(
        extract_select_list_columns(token_range_slice(
            ctx.statement_tokens.as_ref(),
            recent_cte.body_range,
        )),
        vec!["id"]
    );
    assert_eq!(
        resolve_qualifier_tables("recent", &ctx.tables_in_scope),
        vec!["recent".to_string()]
    );
}

#[test]
fn cte_cursor_in_cte_body() {
    let ctx = analyze("WITH temp AS (SELECT | FROM users) SELECT * FROM temp");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn later_cte_is_not_visible_inside_previous_cte_body() {
    let ctx = analyze(
        "WITH c1 AS (SELECT c2.| FROM dual), c2 AS (SELECT 1 AS id FROM dual) SELECT * FROM c1",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        !has_name(&cte_names(&ctx), "C2"),
        "later sibling CTE must not be visible inside an earlier CTE body: {:?}",
        cte_names(&ctx)
    );
    assert!(
        !has_name(&table_names(&ctx), "C2"),
        "later sibling CTE must not enter table scope inside an earlier CTE body: {:?}",
        table_names(&ctx)
    );
}

#[test]
fn recursive_cte_is_visible_inside_its_own_body() {
    let ctx = analyze(
        "WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT r.| FROM r WHERE n < 10) SELECT * FROM r",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "R"),
        "recursive CTE should stay visible inside its own body: {:?}",
        cte_names(&ctx)
    );
    let recursive_cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("r"))
        .expect("expected recursive CTE in scope inside its own body");
    assert_eq!(recursive_cte.explicit_columns, vec!["n"]);
    assert_eq!(
        resolve_qualifier_tables("r", &ctx.tables_in_scope),
        vec!["r".to_string()]
    );
}

#[test]
fn recursive_cte_is_visible_in_recursive_term_before_self_reference_token() {
    let ctx = analyze(
        "WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT | FROM r WHERE n < 10) SELECT * FROM r",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "R"),
        "recursive CTE should be visible as soon as cursor enters recursive term: {:?}",
        cte_names(&ctx)
    );
    assert!(
        has_name(&table_names(&ctx), "R"),
        "recursive CTE should enter table scope inside recursive term: {:?}",
        table_names(&ctx)
    );
}

#[test]
fn recursive_cte_is_not_visible_in_anchor_term_before_set_operator() {
    let ctx =
        analyze("WITH r(n) AS (SELECT | FROM dual UNION ALL SELECT n + 1 FROM r WHERE n < 10) SELECT * FROM r");

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        !has_name(&cte_names(&ctx), "R"),
        "recursive CTE must stay hidden inside anchor term before set operator: {:?}",
        cte_names(&ctx)
    );
    assert!(
        !has_name(&table_names(&ctx), "R"),
        "recursive CTE must stay out of table scope inside anchor term before set operator: {:?}",
        table_names(&ctx)
    );
}

#[test]
fn recursive_cte_is_not_visible_in_nested_anchor_subquery_before_set_operator() {
    let ctx = analyze(
        "WITH r(n) AS (SELECT * FROM (SELECT | FROM dual) anchor_sub UNION ALL SELECT n + 1 FROM r WHERE n < 10) SELECT * FROM r",
    );

    assert_eq!(ctx.depth, 2);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        !has_name(&cte_names(&ctx), "R"),
        "recursive CTE must stay hidden inside nested anchor subquery before recursive term: {:?}",
        cte_names(&ctx)
    );
    assert!(
        !has_name(&table_names(&ctx), "R"),
        "recursive CTE must stay out of nested anchor subquery table scope before recursive term: {:?}",
        table_names(&ctx)
    );
}

#[test]
fn non_recursive_cte_is_not_visible_inside_its_own_body() {
    let ctx = analyze("WITH temp AS (SELECT temp.| FROM users) SELECT * FROM temp");

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        !has_name(&cte_names(&ctx), "TEMP"),
        "non-recursive CTE must not be visible inside its own body: {:?}",
        cte_names(&ctx)
    );
    assert!(
        !has_name(&table_names(&ctx), "TEMP"),
        "non-recursive CTE must not enter table scope inside its own body: {:?}",
        table_names(&ctx)
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| !table.name.eq_ignore_ascii_case("temp")),
        "non-recursive CTE must stay out of visible table scope inside its own body: {:?}",
        ctx.tables_in_scope
    );
}

#[test]
fn cte_with_nested_subquery() {
    let ctx =
        analyze("WITH temp AS (SELECT * FROM (SELECT id FROM inner_t) sub) SELECT | FROM temp");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("temp")));
}

#[test]
fn outer_cte_is_visible_inside_non_lateral_from_subquery() {
    let ctx = analyze(
        "WITH outer_cte AS (SELECT 1 AS id FROM dual) \
         SELECT * FROM (SELECT outer_cte.| FROM outer_cte) sub",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "OUTER_CTE"),
        "outer CTE should remain visible inside a nested FROM subquery: {:?}",
        cte_names(&ctx)
    );
    assert_eq!(
        resolve_qualifier_tables("outer_cte", &ctx.tables_in_scope),
        vec!["outer_cte".to_string()]
    );
}

#[test]
fn outer_cte_is_visible_in_second_set_operator_operand() {
    let ctx = analyze(
        "WITH outer_cte AS (SELECT 1 AS id FROM dual) \
         SELECT id FROM outer_cte UNION ALL SELECT outer_cte.| FROM outer_cte",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "OUTER_CTE"),
        "outer CTE should remain visible in later set-operator operands: {:?}",
        cte_names(&ctx)
    );
    assert_eq!(
        resolve_qualifier_tables("outer_cte", &ctx.tables_in_scope),
        vec!["outer_cte".to_string()]
    );
}

#[test]
fn cte_with_mismatched_close_before_with_is_still_detected() {
    let ctx = analyze(") WITH temp AS (SELECT id FROM users) SELECT | FROM temp");
    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.contains(&"TEMP".to_string()),
        "top-level WITH should be detected after unmatched close paren: {:?}",
        cte_n
    );
}

#[test]
fn cte_with_comment_between_with_and_name_is_detected() {
    let ctx = analyze("WITH /*hint*/ cte AS (SELECT 1 AS n FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.contains(&"CTE".to_string()),
        "CTE name should be detected even with comment after WITH: {:?}",
        cte_n
    );
}

#[test]
fn cte_as_materialized_is_detected() {
    let ctx = analyze("WITH cte AS MATERIALIZED (SELECT 1 AS n FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.contains(&"CTE".to_string()),
        "CTE with AS MATERIALIZED should be detected: {:?}",
        cte_n
    );
}

#[test]
fn cte_as_not_materialized_is_detected() {
    let ctx = analyze("WITH cte AS NOT MATERIALIZED (SELECT 1 AS n FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.contains(&"CTE".to_string()),
        "CTE with AS NOT MATERIALIZED should be detected: {:?}",
        cte_n
    );
}

#[test]
fn cte_with_comment_between_as_and_materialized_is_detected() {
    let ctx =
        analyze("WITH cte AS /*hint*/ MATERIALIZED (SELECT 1 AS n FROM dual) SELECT | FROM cte");
    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.contains(&"CTE".to_string()),
        "CTE with comments around MATERIALIZED should be detected: {:?}",
        cte_n
    );
}

// ─── Complex nested query tests ─────────────────────────────────────────

#[test]
fn nested_subquery_in_where() {
    let ctx = analyze("SELECT * FROM employees WHERE dept_id IN (SELECT | FROM departments)");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn nested_subquery_in_where_from() {
    let ctx = analyze("SELECT * FROM employees WHERE dept_id IN (SELECT dept_id FROM |");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::FromClause);
}

#[test]
fn correlated_subquery() {
    let ctx = analyze(
        "SELECT * FROM employees e WHERE salary > (SELECT AVG(salary) FROM employees e2 WHERE e2.dept_id = e.| )",
    );
    // Cursor is inside the subquery at depth 1
    assert_eq!(ctx.depth, 1);
}

#[test]
fn subquery_in_select_list() {
    let ctx = analyze(
        "SELECT (SELECT | FROM departments d WHERE d.id = e.dept_id) AS dept_name FROM employees e",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn inline_view_with_join() {
    let ctx = analyze(
        "SELECT | FROM (SELECT e.id, d.name FROM employees e JOIN departments d ON e.dept_id = d.id) v",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("v")));
}

#[test]
fn triple_nested_subquery() {
    let ctx = analyze("SELECT * FROM (SELECT * FROM (SELECT | FROM innermost) mid) outer_q");
    assert_eq!(ctx.depth, 2);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

// ─── UNION / set operation tests ─────────────────────────────────────────

#[test]
fn union_resets_phase_for_second_select() {
    let ctx = analyze("SELECT a FROM t1 UNION ALL SELECT | FROM t2");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert_eq!(ctx.depth, 0);
}

#[test]
fn union_collects_tables_from_both_parts() {
    let ctx = analyze("SELECT a FROM t1 UNION ALL SELECT | FROM t2");
    let names = table_names(&ctx);
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn union_second_select_does_not_leak_left_operand_tables_into_scope() {
    let ctx = analyze("SELECT a FROM t1 UNION ALL SELECT | FROM t2");
    let names = table_names(&ctx);

    assert!(
        !names.contains(&"T1".to_string()),
        "left UNION operand tables must not leak into right SELECT scope: {:?}",
        names
    );
    assert!(
        names.contains(&"T2".to_string()),
        "right UNION operand table should remain visible: {:?}",
        names
    );
}

#[test]
fn intersect_second_select_does_not_leak_left_operand_tables_into_scope() {
    let ctx = analyze("SELECT a FROM t1 INTERSECT SELECT | FROM t2");
    let names = table_names(&ctx);

    assert!(
        !names.contains(&"T1".to_string()),
        "left INTERSECT operand tables must not leak into right SELECT scope: {:?}",
        names
    );
    assert!(
        names.contains(&"T2".to_string()),
        "right INTERSECT operand table should remain visible: {:?}",
        names
    );
}

#[test]
fn minus_second_select_does_not_leak_left_operand_tables_into_scope() {
    let ctx = analyze("SELECT a FROM t1 MINUS SELECT | FROM t2");
    let names = table_names(&ctx);

    assert!(
        !names.contains(&"T1".to_string()),
        "left MINUS operand tables must not leak into right SELECT scope: {:?}",
        names
    );
    assert!(
        names.contains(&"T2".to_string()),
        "right MINUS operand table should remain visible: {:?}",
        names
    );
}

#[test]
fn multiset_union_inside_expression_keeps_where_phase_and_table_scope() {
    let ctx = analyze("SELECT * FROM t WHERE nested_col MULTISET UNION DISTINCT | IS NOT NULL");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T".to_string()),
        "MULTISET UNION in expression should not reset statement scope: {:?}",
        names
    );
}

#[test]
fn multiset_except_inside_expression_keeps_where_phase_and_table_scope() {
    let ctx = analyze("SELECT * FROM t WHERE nested_col MULTISET EXCEPT | IS NOT NULL");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T".to_string()),
        "MULTISET EXCEPT in expression should not reset statement scope: {:?}",
        names
    );
}

#[test]
fn multiset_intersect_inside_expression_keeps_where_phase_and_table_scope() {
    let ctx = analyze("SELECT * FROM t WHERE nested_col MULTISET INTERSECT | IS NOT NULL");

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T".to_string()),
        "MULTISET INTERSECT in expression should not reset statement scope: {:?}",
        names
    );
}

// ─── Qualifier resolution tests ──────────────────────────────────────────

#[test]
fn resolve_qualifier_by_alias() {
    let tables = vec![ScopedTableRef {
        name: "employees".to_string(),
        alias: Some("e".to_string()),
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables("e", &tables);
    assert_eq!(result, vec!["employees"]);
}

#[test]
fn resolve_qualifier_by_table_name() {
    let tables = vec![ScopedTableRef {
        name: "employees".to_string(),
        alias: None,
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables("employees", &tables);
    assert_eq!(result, vec!["employees"]);
}

#[test]
fn resolve_qualifier_by_unqualified_name_for_schema_qualified_table() {
    let tables = vec![ScopedTableRef {
        name: "hr.employees".to_string(),
        alias: None,
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables("employees", &tables);
    assert_eq!(result, vec!["hr.employees"]);
}

#[test]
fn resolve_qualifier_case_insensitive() {
    let tables = vec![ScopedTableRef {
        name: "EMPLOYEES".to_string(),
        alias: Some("E".to_string()),
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables("e", &tables);
    assert_eq!(result, vec!["EMPLOYEES"]);
}

#[test]
fn resolve_qualifier_unknown_falls_back() {
    let tables = vec![ScopedTableRef {
        name: "employees".to_string(),
        alias: Some("e".to_string()),
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables("unknown", &tables);
    assert_eq!(result, vec!["unknown"]);
}

#[test]
fn resolve_qualifier_prefers_deeper_alias_scope() {
    let tables = vec![
        ScopedTableRef {
            name: "outer_table".to_string(),
            alias: Some("t".to_string()),
            depth: 0,
            is_cte: false,
        },
        ScopedTableRef {
            name: "inner_table".to_string(),
            alias: Some("t".to_string()),
            depth: 1,
            is_cte: false,
        },
    ];
    let result = resolve_qualifier_tables("t", &tables);
    assert_eq!(result, vec!["inner_table"]);
}

#[test]
fn resolve_qualifier_prefers_inner_alias_in_nested_query() {
    let ctx = analyze("SELECT * FROM outer_t t WHERE EXISTS (SELECT 1 FROM inner_t t WHERE t.|)");
    let result = resolve_qualifier_tables("t", &ctx.tables_in_scope);
    assert_eq!(result, vec!["inner_t"]);
}

#[test]
fn resolve_qualifier_matches_quoted_alias() {
    let tables = vec![ScopedTableRef {
        name: "Emp Table".to_string(),
        alias: Some("e".to_string()),
        depth: 0,
        is_cte: false,
    }];
    let result = resolve_qualifier_tables(r#""e""#, &tables);
    assert_eq!(result, vec!["Emp Table"]);
}

// ─── Comment handling tests ──────────────────────────────────────────────

#[test]
fn comments_dont_affect_phase_detection() {
    let ctx = analyze("SELECT /* this is a comment */ a FROM /* another */ t WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn line_comment_doesnt_affect_phase() {
    let ctx = analyze("SELECT a\n-- comment\nFROM t\nWHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

// ─── String literal handling tests ───────────────────────────────────────

#[test]
fn string_with_keywords_inside() {
    let ctx = analyze("SELECT 'FROM WHERE' FROM t WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"T".to_string()));
}

// ─── Multiple statement boundary tests ───────────────────────────────────

#[test]
fn semicolon_resets_state() {
    let ctx = analyze("SELECT 1 FROM dual; SELECT | FROM t2");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(names.contains(&"T2".to_string()));
    assert!(!names.contains(&"DUAL".to_string()));
}

#[test]
fn trailing_semicolon_preserves_current_statement_table_aliases() {
    let ctx = analyze("SELECT e.| FROM employees e;");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );

    let result = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(result, vec!["employees"]);
}

#[test]
fn trailing_semicolon_preserves_cte_alias_resolution() {
    let ctx = analyze(
        "WITH base AS (SELECT empno FROM emp), filtered AS (SELECT * FROM base) SELECT f.| FROM filtered f;",
    );
    let result = resolve_qualifier_tables("f", &ctx.tables_in_scope);
    assert_eq!(result, vec!["filtered"]);
}

// ─── UPDATE statement tests ──────────────────────────────────────────────

#[test]
fn update_target_table() {
    let ctx = analyze("UPDATE employees SET |");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    let names = table_names(&ctx);
    assert!(names.contains(&"EMPLOYEES".to_string()));
    assert_eq!(ctx.focused_tables, vec!["employees".to_string()]);
}

#[test]
fn update_with_where() {
    let ctx = analyze("UPDATE employees SET salary = 1000 WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn update_with_alias_qualifier_resolution() {
    let ctx = analyze("UPDATE employees e SET e.| = 1000");
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees"]);
}

#[test]
fn update_set_expression_after_equals_returns_expression_phase() {
    let ctx = analyze("UPDATE employees SET salary = |");
    assert_eq!(ctx.phase, SqlPhase::SetClause);
    assert!(ctx.focused_tables.is_empty());
}

// ─── DELETE statement tests ──────────────────────────────────────────────

#[test]
fn delete_from() {
    let ctx = analyze("DELETE FROM employees WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"EMPLOYEES".to_string()));
}

#[test]
fn delete_with_alias_qualifier_resolution() {
    let ctx = analyze("DELETE FROM employees e WHERE e.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees"]);
}

// ─── INSERT statement tests ──────────────────────────────────────────────

#[test]
fn insert_column_list_context_after_target_table() {
    let ctx = analyze("INSERT INTO employees (|) VALUES (1)");
    assert_eq!(ctx.phase, SqlPhase::InsertColumnList);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn insert_values_keeps_target_table_in_scope() {
    let ctx = analyze("INSERT INTO employees (id, name) VALUES (1, |)");
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn insert_select_keeps_target_and_source_tables_in_scope() {
    let ctx = analyze("INSERT INTO audit_emp (emp_id) SELECT e.| FROM employees e");
    assert_eq!(ctx.phase, SqlPhase::SelectList);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"AUDIT_EMP".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees"]);
}

#[test]
fn insert_subquery_in_values_increases_query_depth() {
    let ctx = analyze("INSERT INTO employees (id) VALUES ((SELECT | FROM dual))");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn insert_subquery_depth_returns_to_zero_after_closing_values_subquery() {
    let ctx =
        analyze("INSERT INTO employees (id) VALUES ((SELECT 1 FROM dual)) RETURNING | INTO :id");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn insert_all_second_into_is_table_context() {
    let ctx = analyze("INSERT ALL INTO emp_a (id) VALUES (1) INTO | SELECT 1 FROM dual");
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn insert_all_collects_all_targets() {
    let ctx = analyze(
        "INSERT ALL INTO emp_a (id) VALUES (1) INTO emp_b (id) VALUES (2) SELECT | FROM dual",
    );
    assert_eq!(ctx.phase, SqlPhase::SelectList);

    let names = table_names(&ctx);
    assert!(names.contains(&"EMP_A".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"EMP_B".to_string()), "tables: {:?}", names);
}

#[test]
fn select_into_does_not_leak_into_next_select_in_package_body() {
    let ctx = analyze(
        r#"create package body a as
procedure b (c in number) as
begin
    select d
    into e
    from f;
    select |
    from h;
end;
end;"#,
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let names = table_names(&ctx);
    assert!(has_name(&names, "h"), "tables: {:?}", names);
    assert!(!has_name(&names, "e"), "tables: {:?}", names);
    assert!(!has_name(&names, "f"), "tables: {:?}", names);
}

#[test]
fn select_into_target_is_not_collected_as_table() {
    let ctx = analyze(
        r#"begin
    select d
    into e
    from f
    where |;
end;"#,
    );

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(has_name(&names, "f"), "tables: {:?}", names);
    assert!(!has_name(&names, "e"), "tables: {:?}", names);
}

#[test]
fn bulk_collect_into_target_is_not_collected_as_table() {
    let ctx = analyze(
        r#"begin
    select empno
    bulk collect into l_empnos
    from emp
    where |;
end;"#,
    );

    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(has_name(&names, "emp"), "tables: {:?}", names);
    assert!(!has_name(&names, "l_empnos"), "tables: {:?}", names);
}

#[test]
fn insert_all_second_into_column_list_is_column_context() {
    let ctx = analyze(
        "INSERT ALL INTO emp_a (id) VALUES (1) INTO emp_b (|) VALUES (2) SELECT 1 FROM dual",
    );
    assert_eq!(ctx.phase, SqlPhase::InsertColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_not_matched_insert_column_list_is_column_context() {
    let ctx = analyze(
        "MERGE INTO emp t USING src s ON (t.empno = s.empno) WHEN NOT MATCHED THEN INSERT (|) VALUES (s.empno)",
    );
    assert_eq!(ctx.phase, SqlPhase::MergeInsertColumnList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn dblink_table_reference_keeps_alias_in_scope() {
    let ctx = analyze("SELECT e.| FROM scott.emp@hr_link e WHERE e.empno = 10");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("SCOTT.EMP@HR_LINK")
                && table.alias.as_deref() == Some("e")),
        "db link table reference should keep alias visibility: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn insert_first_second_into_is_table_context() {
    let ctx = analyze(
        "INSERT FIRST WHEN 1 = 1 THEN INTO emp_a (id) VALUES (1) INTO | SELECT 1 FROM dual",
    );
    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

// ─── Complex real-world query tests ─────────────────────────────────────

#[test]
fn complex_cte_with_join_and_subquery() {
    let ctx = analyze(
        "WITH dept_stats AS (\
            SELECT dept_id, COUNT(*) cnt FROM employees GROUP BY dept_id\
         ), \
         salary_stats AS (\
            SELECT dept_id, AVG(salary) avg_sal FROM employees GROUP BY dept_id\
         ) \
         SELECT d.dept_name, ds.cnt, ss.avg_sal \
         FROM departments d \
         JOIN dept_stats ds ON d.id = ds.dept_id \
         JOIN salary_stats ss ON d.id = ss.dept_id \
         WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert_eq!(ctx.depth, 0);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("dept_stats")),
        "CTE dept_stats should be in scope: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("salary_stats")),
        "CTE salary_stats should be in scope: {:?}",
        names
    );
}

#[test]
fn oracle_hierarchical_query() {
    let ctx = analyze(
        "SELECT employee_id, manager_id, LEVEL \
         FROM employees \
         START WITH manager_id IS NULL \
         CONNECT BY |",
    );
    assert_eq!(ctx.phase, SqlPhase::ConnectByClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"EMPLOYEES".to_string()));
}

#[test]
fn from_clause_with_function_call_in_select() {
    // Ensure parentheses in function calls don't confuse depth tracking
    let ctx = analyze("SELECT NVL(a, 0), COALESCE(b, c, d) FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert_eq!(ctx.depth, 0);
}

#[test]
fn select_function_arg_cursor_is_column_context() {
    let ctx = analyze("SELECT MAX(|) FROM HELP");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
    let names = table_names(&ctx);
    assert!(names.contains(&"HELP".to_string()));
}

#[test]
fn select_function_arg_cursor_with_missing_paren_is_column_context() {
    let ctx = analyze("SELECT MAX(| FROM HELP");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
    let names = table_names(&ctx);
    assert!(names.contains(&"HELP".to_string()));
}

#[test]
fn case_expression_in_select_list() {
    let ctx = analyze("SELECT CASE WHEN a = 1 THEN 'x' ELSE 'y' END, | FROM t");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert_eq!(ctx.depth, 0);
}

#[test]
fn subquery_in_from_with_join_after() {
    let ctx = analyze(
        "SELECT * FROM (SELECT id FROM t1) sub \
         JOIN t2 ON sub.id = t2.id \
         WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("sub")),
        "tables: {:?}",
        names
    );
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn multiple_subqueries_in_from() {
    let ctx = analyze(
        "SELECT * FROM \
         (SELECT id FROM t1) a, \
         (SELECT id FROM t2) b \
         WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("a")),
        "tables: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("b")),
        "tables: {:?}",
        names
    );
}

#[test]
fn cte_used_multiple_times() {
    let ctx = analyze(
        "WITH temp AS (SELECT id FROM users) \
         SELECT * FROM temp t1 JOIN temp t2 ON t1.id = t2.id WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("temp")));
}

#[test]
fn exists_subquery() {
    let ctx = analyze(
        "SELECT * FROM employees e WHERE EXISTS (SELECT 1 FROM departments d WHERE d.id = e.|)",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn in_subquery_from_clause_tables() {
    let ctx = analyze(
        "SELECT * FROM employees WHERE dept_id IN (SELECT dept_id FROM departments WHERE |)",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    // Inside the subquery, departments should be visible
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
    // employees from outer query should also be visible (ancestor scope visibility)
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

// ─── Edge cases ──────────────────────────────────────────────────────────

#[test]
fn empty_from_clause() {
    let ctx = analyze("SELECT 1 FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn cursor_right_after_select() {
    let ctx = analyze("SELECT|");
    // After SELECT keyword, we should be in SelectList
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn cursor_in_from_before_any_table() {
    let ctx = analyze("SELECT a FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn left_outer_join() {
    let ctx =
        analyze("SELECT | FROM employees e LEFT OUTER JOIN departments d ON e.dept_id = d.id");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"DEPARTMENTS".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn cross_join() {
    let ctx = analyze("SELECT | FROM t1 CROSS JOIN t2");
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn natural_join() {
    let ctx = analyze("SELECT | FROM t1 NATURAL JOIN t2");
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn natural_left_outer_join_keeps_both_relations_in_scope() {
    let ctx = analyze("SELECT d.| FROM emp e NATURAL LEFT OUTER JOIN dept d");
    let names = table_names(&ctx);
    assert!(names.contains(&"EMP".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"DEPT".to_string()), "tables: {:?}", names);
}

#[test]
fn natural_right_join_keeps_both_relations_in_scope() {
    let ctx = analyze("SELECT e.| FROM emp e NATURAL RIGHT JOIN dept d");
    let names = table_names(&ctx);
    assert!(names.contains(&"EMP".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"DEPT".to_string()), "tables: {:?}", names);
}

#[test]
fn natural_full_outer_join_keeps_both_relations_in_scope() {
    let ctx = analyze("SELECT d.| FROM emp e NATURAL FULL OUTER JOIN dept d");
    let names = table_names(&ctx);
    assert!(names.contains(&"EMP".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"DEPT".to_string()), "tables: {:?}", names);
}

#[test]
fn partitioned_outer_join_does_not_treat_partition_keyword_as_alias() {
    let ctx = analyze(
        "SELECT * FROM sales s PARTITION BY (s.region_id) RIGHT OUTER JOIN targets t ON s.region_id = t.region_id WHERE t.|",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(aliases.iter().all(|alias| alias != "PARTITION"));
    assert!(
        aliases.iter().any(|alias| alias == "S"),
        "aliases: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "T"),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn table_function_alias_column_list_keeps_alias_visible_in_where_clause() {
    let ctx = analyze("SELECT * FROM TABLE(get_rows()) r(id, val) WHERE r.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().any(|alias| alias == "R"),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn table_function_alias_column_list_before_join_keeps_following_join_relation_visible() {
    let ctx =
        analyze("SELECT * FROM TABLE(get_rows()) r(id, val) JOIN dept d ON d.id = r.id WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let names = table_names(&ctx);
    assert!(names.contains(&"DEPT".to_string()), "tables: {:?}", names);
    assert!(
        names.contains(&"GET_ROWS".to_string()),
        "tables: {:?}",
        names
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().any(|alias| alias == "R"),
        "aliases: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "aliases: {:?}",
        aliases
    );
    assert!(
        aliases.iter().all(|alias| alias != "ID" && alias != "VAL"),
        "alias column-list identifiers must not leak into relation aliases: {:?}",
        aliases
    );
}

#[test]
fn table_function_alias_column_list_before_comma_keeps_following_relation_visible() {
    let ctx = analyze("SELECT * FROM TABLE(get_rows()) r(id, val), dept d WHERE d.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let names = table_names(&ctx);
    assert!(
        names.contains(&"GET_ROWS".to_string()),
        "tables: {:?}",
        names
    );
    assert!(names.contains(&"DEPT".to_string()), "tables: {:?}", names);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().any(|alias| alias == "R"),
        "aliases: {:?}",
        aliases
    );
    assert!(
        aliases.iter().any(|alias| alias == "D"),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn lateral_subquery_can_see_outer_table_scope() {
    let ctx = analyze("SELECT * FROM t1 a, LATERAL (SELECT a.| FROM t2 b) l");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T1".to_string()),
        "lateral subquery should inherit outer scope table: {:?}",
        names
    );
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn lateral_subquery_with_comment_before_open_paren_keeps_outer_scope() {
    let ctx = analyze("SELECT * FROM t1 a, LATERAL /* keep */ (SELECT a.| FROM t2 b) l");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T1".to_string()),
        "lateral subquery should inherit outer scope table even with comment: {:?}",
        names
    );
    assert!(names.contains(&"T2".to_string()), "tables: {:?}", names);
}

#[test]
fn lateral_table_function_argument_can_see_outer_table_scope() {
    let ctx = analyze("SELECT * FROM t1 a, LATERAL JSON_TABLE(a.payload, '$' COLUMNS (id NUMBER PATH '$.id')) jt WHERE a.|");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"T1".to_string()),
        "lateral table function should inherit outer scope table: {:?}",
        names
    );
    assert!(
        names.contains(&"JT".to_string()),
        "table function alias should remain visible: {:?}",
        names
    );
}

#[test]
fn cross_apply_table_function_argument_can_see_outer_table_scope() {
    let ctx = analyze("SELECT * FROM t1 a CROSS APPLY OPENJSON(a.payload) oj WHERE a.|");
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(
        names.contains(&"OPENJSON".to_string()),
        "tables: {:?}",
        names
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.contains(&"OJ".to_string()),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn outer_apply_table_function_alias_is_collected() {
    let ctx = analyze("SELECT * FROM t1 a OUTER APPLY OPENJSON(a.payload) oj WHERE oj.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.contains(&"OJ".to_string()),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn join_lateral_unknown_table_function_alias_is_collected() {
    let ctx =
        analyze("SELECT * FROM t1 a JOIN LATERAL custom_table_fn(a.id) cf ON 1 = 1 WHERE cf.|");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.contains(&"CF".to_string()),
        "aliases: {:?}",
        aliases
    );
}

#[test]
fn implicit_lateral_set_returning_function_argument_can_see_left_relation_scope() {
    let ctx = analyze("SELECT * FROM orders o, generate_series(1, o.|) gs");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "implicit lateral set-returning function arguments should keep left relation visible: {:?}",
        names
    );
}

#[test]
fn implicit_lateral_set_returning_function_alias_is_collected() {
    let ctx = analyze("SELECT gs.| FROM orders o, generate_series(1, o.max_n) gs");

    assert!(
        ctx.tables_in_scope.iter().any(|table| {
            table.name.eq_ignore_ascii_case("generate_series")
                && table.alias.as_deref() == Some("gs")
        }),
        "set-returning function alias should be collected after argument list: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn lateral_keyword_is_not_parsed_as_left_table_alias() {
    let ctx = analyze("SELECT * FROM t1 LATERAL (SELECT * FROM t2) l WHERE l.|");
    let aliases: Vec<&str> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_deref())
        .collect();

    assert!(
        aliases
            .iter()
            .all(|alias| !alias.eq_ignore_ascii_case("LATERAL")),
        "LATERAL must remain a join modifier, not a relation alias: {:?}",
        aliases
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("l")),
        "lateral subquery alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sample_clause_keyword_is_not_parsed_as_table_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp SAMPLE (10) WHERE |");
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SAMPLE")),
        "SAMPLE clause keyword must not become alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sample_block_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp SAMPLE BLOCK (10) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("BLOCK")),
        "SAMPLE BLOCK clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following SAMPLE BLOCK clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sample_block_seed_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp SAMPLE BLOCK (10) SEED (7) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("BLOCK")),
        "SAMPLE BLOCK clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SEED")),
        "SAMPLE SEED clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following SAMPLE BLOCK ... SEED clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sample_bernoulli_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp SAMPLE BERNOULLI (10) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("BERNOULLI")),
        "SAMPLE BERNOULLI clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following SAMPLE BERNOULLI clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sample_system_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp SAMPLE SYSTEM (10) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SYSTEM")),
        "SAMPLE SYSTEM clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following SAMPLE SYSTEM clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_keyword_is_not_parsed_as_table_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp PARTITION (p_202401) WHERE |");
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("PARTITION")),
        "PARTITION clause keyword must not become alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_for_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM sales PARTITION FOR (DATE '2024-01-01') s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("FOR")),
        "PARTITION FOR clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following PARTITION FOR clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn subpartition_for_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM sales SUBPARTITION FOR (1, 2) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("FOR")),
        "SUBPARTITION FOR clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following SUBPARTITION FOR clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_clause_before_pivot_still_collects_derived_alias() {
    let ctx = analyze(
        "SELECT p.| FROM sales PARTITION (p202401) PIVOT (SUM(amount) FOR quarter IN ('Q1' AS q1_amount)) p",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("p")),
        "derived alias after PARTITION + PIVOT should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_clause_before_match_recognize_still_collects_derived_alias() {
    let ctx = analyze(
        "SELECT mr.| FROM sales PARTITION (p202401) MATCH_RECOGNIZE (PARTITION BY deptno ORDER BY amount PATTERN (a) DEFINE a AS amount > 0) mr",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("mr")),
        "derived alias after PARTITION + MATCH_RECOGNIZE should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_clause_before_model_still_collects_derived_alias() {
    let ctx = analyze(
        "SELECT m.| FROM sales PARTITION (p202401) MODEL DIMENSION BY (deptno) MEASURES (amount) RULES (amount[ANY] = amount[CV()]) m",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("m")),
        "derived alias after PARTITION + MODEL should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn tablesample_keyword_is_not_parsed_as_table_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp TABLESAMPLE (10) WHERE |");
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("TABLESAMPLE")),
        "TABLESAMPLE clause keyword must not become alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn cross_apply_subquery_can_see_outer_table_scope() {
    let ctx = analyze("SELECT * FROM oqt_t_emp jt CROSS APPLY (SELECT jt.| FROM dual) it");
    let names = table_names(&ctx);
    assert!(
        names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_emp")),
        "cross apply subquery should inherit outer scope table: {:?}",
        names
    );
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case("dual")),
        "cross apply subquery should keep local table scope: {:?}",
        names
    );
}

#[test]
fn outer_apply_subquery_can_see_outer_table_scope() {
    let ctx = analyze("SELECT * FROM oqt_t_emp jt OUTER APPLY (SELECT jt.| FROM dual) it");
    let names = table_names(&ctx);
    assert!(
        names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_emp")),
        "outer apply subquery should inherit outer scope table: {:?}",
        names
    );
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case("dual")),
        "outer apply subquery should keep local table scope: {:?}",
        names
    );
}

#[test]
fn cross_apply_subquery_exposes_alias_in_outer_scope() {
    let ctx = analyze("SELECT a.| FROM t1 CROSS APPLY (SELECT id FROM t2) a");
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"A".to_string()), "tables: {:?}", names);
}

#[test]
fn outer_apply_keeps_from_phase_before_right_relation() {
    let ctx = analyze("SELECT * FROM t1 OUTER APPLY |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn apply_subquery_keeps_outer_scope_visibility() {
    let ctx = analyze("SELECT * FROM t1 APPLY (SELECT t1.| FROM dual) a");
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
}

#[test]
fn only_wrapper_relation_is_collected_and_visible() {
    let ctx = analyze("SELECT o.| FROM ONLY (hr.orders) o");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.ORDERS".to_string()),
        "ONLY wrapper should preserve underlying relation name: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("o")),
        "ONLY wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn table_wrapper_relation_with_identifier_argument_is_collected() {
    let ctx = analyze("SELECT c.| FROM TABLE(hr.order_rows) c");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.ORDER_ROWS".to_string()),
        "TABLE wrapper should preserve identifier-like relation path: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "TABLE wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_wrapper_relation_with_identifier_argument_is_collected() {
    let ctx = analyze("SELECT c.| FROM THE(hr.order_rows) c");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.ORDER_ROWS".to_string()),
        "THE wrapper should preserve identifier-like relation path: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "THE wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn containers_wrapper_relation_with_identifier_argument_is_collected() {
    let ctx = analyze("SELECT c.| FROM CONTAINERS(hr.orders) c");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.ORDERS".to_string()),
        "CONTAINERS wrapper should preserve identifier-like relation path: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "CONTAINERS wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn shards_wrapper_relation_with_identifier_argument_is_collected() {
    let ctx = analyze("SELECT s.| FROM SHARDS(hr.orders) s");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"HR.ORDERS".to_string()),
        "SHARDS wrapper should preserve identifier-like relation path: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "SHARDS wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn table_wrapper_collection_expression_keeps_alias() {
    let ctx = analyze("SELECT c.| FROM TABLE(get_rows()) c");
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "TABLE(collection_expression) should still allow alias-driven completion: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_wrapper_collection_expression_keeps_alias() {
    let ctx = analyze("SELECT c.| FROM THE(get_rows()) c");
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "THE(collection_expression) should still allow alias-driven completion: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rows_from_wrapper_relation_keeps_alias() {
    let ctx = analyze("SELECT rf.| FROM ROWS FROM (generate_series(1, 2)) AS rf");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("rf")),
        "ROWS FROM wrapper alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn lateral_rows_from_wrapper_keeps_left_relation_visible() {
    let ctx =
        analyze("SELECT * FROM orders o, LATERAL ROWS FROM (expand_order(o.id)) rf WHERE o.|");

    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "left relation should remain visible around LATERAL ROWS FROM: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("rf")),
        "LATERAL ROWS FROM alias should be captured: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn implicit_lateral_rows_from_function_argument_can_see_left_relation_scope() {
    let ctx = analyze("SELECT * FROM orders o, ROWS FROM (generate_series(1, o.|)) rf");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"ORDERS".to_string()),
        "implicit lateral ROWS FROM function arguments should keep left relation visible: {:?}",
        names
    );
}

#[test]
fn parenthesized_table_wrapper_relation_keeps_alias() {
    let ctx = analyze("SELECT c.| FROM (TABLE(get_rows())) c");

    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("c")),
        "parenthesized TABLE(...) relation should keep alias visibility: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn partition_extension_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM sales PARTITION (p202401) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("PARTITION")),
        "PARTITION clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following PARTITION clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn with_clause_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees WITH (NOLOCK) e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("WITH")),
        "WITH clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following WITH (...) clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn with_clause_without_alias_does_not_capture_hint_as_alias() {
    let ctx = analyze("SELECT * FROM employees WITH (INDEX(idx_emp_name)) WHERE |");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref().is_none()),
        "WITH (...) tokens must not be captured as aliases when no alias exists: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees AS OF SCN (12345) e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("AS")),
        "AS OF clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_timestamp_interval_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees AS OF TIMESTAMP SYSTIMESTAMP - INTERVAL '1' HOUR e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("INTERVAL")),
        "interval keyword must not be captured as alias in flashback clause: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF TIMESTAMP ... INTERVAL clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_interval_bounds_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees VERSIONS BETWEEN TIMESTAMP SYSTIMESTAMP - INTERVAL '1' DAY AND SYSTIMESTAMP e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("INTERVAL")),
        "interval keyword must not be captured as alias in versions clause: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS BETWEEN ... INTERVAL clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees VERSIONS BETWEEN SCN 1 AND 10 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("VERSIONS")),
        "VERSIONS clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_period_for_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees AS OF PERIOD FOR valid_time (TIMESTAMP '2025-01-01 00:00:00') e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("AS")),
        "AS OF PERIOD FOR clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF PERIOD FOR clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_snapshot_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees AS OF SNAPSHOT snap_20250201 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SNAPSHOT")),
        "AS OF SNAPSHOT clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("snap_20250201")),
        "AS OF SNAPSHOT bound identifier must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF SNAPSHOT clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_snapshot_bounds_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees VERSIONS BETWEEN SNAPSHOT snap_old AND SNAPSHOT snap_new e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SNAPSHOT")),
        "VERSIONS BETWEEN SNAPSHOT clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope.iter().all(|table| {
            table.alias.as_deref() != Some("snap_old") && table.alias.as_deref() != Some("snap_new")
        }),
        "VERSIONS BETWEEN SNAPSHOT bound identifiers must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS BETWEEN SNAPSHOT clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_period_for_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees VERSIONS PERIOD FOR valid_time BETWEEN TIMESTAMP '2024-01-01 00:00:00' AND TIMESTAMP '2024-12-31 23:59:59' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("VERSIONS")),
        "VERSIONS PERIOD FOR clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS PERIOD FOR clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_with_comment_before_of_keeps_alias_visible() {
    let ctx = analyze("SELECT * FROM employees AS /* keep */ OF SCN 12345 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("OF")),
        "AS OF clause keyword must not be captured as alias when comment is interleaved: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS /*...*/ OF clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_period_for_scn_bounds_keeps_alias_visible() {
    let ctx = analyze(
        "SELECT * FROM employees VERSIONS PERIOD FOR valid_time BETWEEN SCN MINVALUE AND SCN MAXVALUE e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("MAXVALUE")),
        "SCN bound keywords must not be captured as aliases in VERSIONS PERIOD FOR clause: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS PERIOD FOR ... SCN bounds should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_scn_multiplicative_expression_keeps_alias_visible() {
    let ctx = analyze("SELECT * FROM employees AS OF SCN 100 * 2 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "AS OF SCN expression tokens must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF SCN multiplicative expression should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_scn_multiplicative_bounds_keep_alias_visible() {
    let ctx =
        analyze("SELECT * FROM employees VERSIONS BETWEEN SCN 1 * 2 AND SCN 3 * 4 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "VERSIONS SCN bound tokens must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS BETWEEN SCN multiplicative bounds should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_scn_signed_bound_keeps_alias_visible() {
    let ctx = analyze("SELECT * FROM employees AS OF SCN -100 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "AS OF SCN signed bound tokens must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF SCN signed bound should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_as_of_scn_positional_bind_keeps_alias_visible() {
    let ctx = analyze("SELECT * FROM employees AS OF SCN ? e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "AS OF SCN positional bind must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following AS OF SCN positional bind should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_scn_signed_bounds_keep_alias_visible() {
    let ctx = analyze("SELECT * FROM employees VERSIONS BETWEEN SCN -1 AND SCN +2 e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "VERSIONS SCN signed bound tokens must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS BETWEEN SCN signed bounds should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flashback_versions_between_scn_positional_bind_bounds_keep_alias_visible() {
    let ctx = analyze("SELECT * FROM employees VERSIONS BETWEEN SCN ? AND SCN ? e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SCN")),
        "VERSIONS SCN positional bind tokens must not be captured as aliases: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("e")),
        "alias following VERSIONS BETWEEN SCN positional binds should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn tablesample_repeatable_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM sales TABLESAMPLE BERNOULLI (10) REPEATABLE (7) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("TABLESAMPLE")),
        "TABLESAMPLE clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following TABLESAMPLE REPEATABLE clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn tablesample_seed_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM sales TABLESAMPLE BERNOULLI (10) SEED (7) s WHERE s.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("TABLESAMPLE")),
        "TABLESAMPLE clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SEED")),
        "TABLESAMPLE SEED clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following TABLESAMPLE SEED clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn tablesample_repeatable_and_seed_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM sales TABLESAMPLE BERNOULLI (10) REPEATABLE (3) SEED (7) s WHERE s.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("TABLESAMPLE")),
        "TABLESAMPLE clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("REPEATABLE")),
        "TABLESAMPLE REPEATABLE clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SEED")),
        "TABLESAMPLE SEED clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.alias.as_deref() == Some("s")),
        "alias following TABLESAMPLE REPEATABLE ... SEED clause should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn table_alias_after_as_of_timestamp_clause_is_collected() {
    let ctx =
        analyze("SELECT e.| FROM employees AS OF TIMESTAMP (SYSTIMESTAMP - INTERVAL '1' DAY) e");

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMPLOYEES"),
        "expected employees table in scope, got {:?}",
        names
    );
    assert_eq!(ctx.qualifier_tables, Vec::<String>::new());
    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn table_alias_after_as_of_scn_clause_is_collected() {
    let ctx = analyze("SELECT e.| FROM employees AS OF SCN 12345 e");

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn table_alias_after_as_of_period_for_clause_is_collected() {
    let ctx = analyze("SELECT e.| FROM employees AS OF PERIOD FOR valid_time (SYSTIMESTAMP) e");

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_system_time_as_of_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR SYSTEM_TIME AS OF TIMESTAMP '2025-01-01 00:00:00' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SYSTEM_TIME")),
        "FOR SYSTEM_TIME clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_system_time_between_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR SYSTEM_TIME BETWEEN TIMESTAMP '2024-01-01 00:00:00' AND TIMESTAMP '2024-12-31 23:59:59' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SYSTEM_TIME")),
        "FOR SYSTEM_TIME clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_system_time_from_to_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR SYSTEM_TIME FROM TIMESTAMP '2024-01-01 00:00:00' TO TIMESTAMP '2024-12-31 23:59:59' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SYSTEM_TIME")),
        "FOR SYSTEM_TIME clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_system_time_all_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze("SELECT * FROM employees FOR SYSTEM_TIME ALL e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("SYSTEM_TIME")),
        "FOR SYSTEM_TIME clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_application_time_contained_in_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR APPLICATION_TIME CONTAINED IN (DATE '2024-01-01', DATE '2024-12-31') e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("APPLICATION_TIME")),
        "FOR APPLICATION_TIME clause keyword must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_named_period_as_of_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR valid_time AS OF TIMESTAMP '2025-01-01 00:00:00' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("VALID_TIME")),
        "FOR <period> clause period name must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_named_period_between_before_alias_is_not_parsed_as_alias() {
    let ctx = analyze(
        "SELECT * FROM employees FOR business_time BETWEEN TIMESTAMP '2024-01-01 00:00:00' AND TIMESTAMP '2024-12-31 23:59:59' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("BUSINESS_TIME")),
        "FOR <period> clause period name must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_named_period_as_of_date_literal_keeps_alias_visible() {
    let ctx = analyze("SELECT * FROM employees FOR valid_time AS OF DATE '2025-01-01' e WHERE e.|");

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("VALID_TIME")),
        "FOR <period> AS OF DATE clause period name must not be captured as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

#[test]
fn system_versioning_for_named_period_between_date_literals_keeps_alias_visible() {
    let ctx = analyze(
        "SELECT * FROM employees FOR valid_time BETWEEN DATE '2024-01-01' AND DATE '2024-12-31' e WHERE e.|",
    );

    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|table| table.alias.as_deref() != Some("VALID_TIME")),
        "FOR <period> BETWEEN DATE literals must not capture period name as alias: {:?}",
        ctx.tables_in_scope
    );

    let resolved = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(resolved, vec!["employees".to_string()]);
}

// ─── CTE inside subquery edge case ──────────────────────────────────────

#[test]
fn cte_with_subquery_alias_in_main_query() {
    let ctx = analyze(
        "WITH base AS (SELECT * FROM employees) \
         SELECT * FROM (SELECT id FROM base) sub WHERE sub.|",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("sub")),
        "tables: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("base")),
        "tables: {:?}",
        names
    );
}

#[test]
fn nested_with_inside_subquery_is_not_collected_as_top_level_cte() {
    let ctx = analyze(
        "SELECT * FROM (WITH inner_cte AS (SELECT 1 AS n FROM dual) SELECT n FROM inner_cte) sub WHERE |",
    );
    let cte_n = cte_names(&ctx);
    assert!(
        !cte_n.contains(&"INNER_CTE".to_string()),
        "top-level CTEs should not include nested WITH definitions: {:?}",
        cte_n
    );
}

#[test]
fn depth_one_in_nested_with_subquery_select_list() {
    let ctx = analyze(
        "SELECT * FROM (WITH inner_cte AS (SELECT 1 AS n FROM dual) SELECT | FROM inner_cte) sub",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn depth_zero_after_nested_with_subquery_closes() {
    let ctx = analyze(
        "SELECT * FROM (WITH inner_cte AS (SELECT 1 AS n FROM dual) SELECT n FROM inner_cte) sub WHERE |",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn nested_with_multiple_ctes_after_comma_tracks_cte_state() {
    let ctx = analyze(
        "SELECT * FROM (WITH c1 AS (SELECT 1 AS id FROM dual), c2 AS (SELECT id FROM c1) SELECT | FROM c2) sub",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn nested_with_multiple_ctes_exposes_second_cte_table() {
    let ctx = analyze(
        "SELECT * FROM (WITH c1 AS (SELECT 1 AS id FROM dual), c2 AS (SELECT id FROM c1) SELECT * FROM c2 WHERE c2.|) sub",
    );

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "C2"),
        "expected second nested CTE to remain in scope, got {:?}",
        names
    );
}

#[test]
fn nested_with_xmlnamespaces_clause_keeps_nested_query_depth_without_cte_state() {
    let ctx = analyze(
        "SELECT * FROM (WITH XMLNAMESPACES (DEFAULT | 'urn:emp') SELECT 1 AS id FROM dual) sub",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(
        ctx.ctes.is_empty(),
        "nested WITH XMLNAMESPACES must not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn nested_with_change_tracking_context_clause_keeps_nested_query_depth_without_cte_state() {
    let ctx = analyze(
        "SELECT * FROM (WITH CHANGE_TRACKING_CONTEXT (| 0x01) SELECT 1 AS id FROM dual) sub",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::Initial);
    assert!(
        ctx.ctes.is_empty(),
        "nested WITH CHANGE_TRACKING_CONTEXT must not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn nested_with_in_where_subquery_cte_body_depth_counts_parent_query() {
    let ctx = analyze(
        "SELECT * FROM outer_t o WHERE o.id IN (WITH cte AS (SELECT | FROM inner_t) SELECT id FROM cte)",
    );
    assert_eq!(ctx.depth, 2);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn nested_with_in_where_subquery_main_select_depth_is_one() {
    let ctx = analyze(
        "SELECT * FROM outer_t o WHERE o.id IN (WITH cte AS (SELECT 1 AS id FROM inner_t) SELECT | FROM cte)",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn malformed_with_missing_as_in_query_recovers_depth_and_phase() {
    let ctx = analyze("WITH cte (SELECT 1) SELECT | FROM cte");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn malformed_with_missing_as_in_query_recovers_from_clause() {
    let ctx = analyze("WITH cte (SELECT 1) SELECT * FROM |");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::FromClause);
}

#[test]
fn malformed_with_missing_as_in_subquery_keeps_nested_depth() {
    let ctx = analyze("SELECT * FROM (WITH cte (SELECT 1) SELECT | FROM cte) x");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn sibling_subquery_tables_are_not_visible_inside_current_subquery() {
    let ctx = analyze(
        "SELECT * \
         FROM (SELECT a.id FROM t1 a WHERE a.|) sub1, \
              (SELECT b.id FROM t2 b) sub2",
    );
    let names = table_names(&ctx);
    assert!(names.contains(&"T1".to_string()), "tables: {:?}", names);
    assert!(
        !names.contains(&"T2".to_string()),
        "sibling subquery table should not leak into current scope: {:?}",
        names
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("sub2")),
        "sibling subquery alias should not leak into current scope: {:?}",
        names
    );
}

// ─── Resolve all scope tables ────────────────────────────────────────────

#[test]
fn resolve_all_deduplicates() {
    let tables = vec![
        ScopedTableRef {
            name: "employees".to_string(),
            alias: Some("e".to_string()),
            depth: 0,
            is_cte: false,
        },
        ScopedTableRef {
            name: "employees".to_string(),
            alias: Some("e2".to_string()),
            depth: 0,
            is_cte: false,
        },
    ];
    let result = resolve_all_scope_tables(&tables);
    assert_eq!(result.len(), 1);
}

// ─── MERGE statement ─────────────────────────────────────────────────────

#[test]
fn merge_target_table() {
    let ctx = analyze("MERGE INTO target_table t USING |");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"TARGET_TABLE".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn merge_using_source_table_is_collected() {
    let ctx = analyze(
        "MERGE INTO target_table t USING source_table s ON t.id = s.id \
         WHEN MATCHED THEN UPDATE SET t.val = s.val WHERE |",
    );
    let names = table_names(&ctx);
    assert!(
        names.contains(&"TARGET_TABLE".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"SOURCE_TABLE".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn merge_using_source_without_alias_does_not_capture_when_keyword_as_alias() {
    let ctx = analyze(
        "MERGE INTO target_table t USING source_table ON t.id = source_table.id \
         WHEN MATCHED THEN UPDATE SET t.val = source_table.val WHERE |",
    );

    let source = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("source_table"));
    assert!(source.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        source
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("WHEN")),
        "source table alias must not be parsed as WHEN: {:?}",
        source
    );
}

#[test]
fn merge_using_source_without_alias_does_not_capture_when_not_matched_as_alias() {
    let ctx = analyze(
        "MERGE INTO target_table t USING source_table ON t.id = source_table.id \
         WHEN NOT MATCHED THEN INSERT (id) VALUES (source_table.id) WHERE |",
    );

    let source = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("source_table"));
    assert!(source.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        source
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("WHEN")),
        "source table alias must not be parsed as WHEN: {:?}",
        source
    );
}

#[test]
fn delete_using_source_without_alias_does_not_capture_when_keyword_as_alias() {
    let ctx = analyze(
        "DELETE FROM target_table t USING source_table \
         WHERE t.id = source_table.id RETURNING t.id INTO :id WHEN |",
    );

    let source = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("source_table"));
    assert!(source.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        source
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("WHEN")),
        "source table alias must not be parsed as WHEN: {:?}",
        source
    );
}

#[test]
fn merge_using_phase_is_table_context() {
    let ctx = analyze("MERGE INTO target_table t USING |");
    assert!(ctx.phase.is_table_context());
}

#[test]
fn delete_using_phase_is_table_context() {
    let ctx = analyze("DELETE FROM target_table t USING |");
    assert!(ctx.phase.is_table_context());
}

#[test]
fn delete_using_source_table_is_collected() {
    let ctx = analyze("DELETE FROM target_table t USING source_table s WHERE t.id = s.id AND |");
    let names = table_names(&ctx);
    assert!(
        names.contains(&"TARGET_TABLE".to_string()),
        "tables: {:?}",
        names
    );
    assert!(
        names.contains(&"SOURCE_TABLE".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn delete_using_source_without_alias_does_not_capture_where_keyword_as_alias() {
    let ctx =
        analyze("DELETE FROM target_table t USING source_table WHERE t.id = source_table.id AND |");

    let source = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("source_table"));
    assert!(source.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        source
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("WHERE")),
        "source table alias must not be parsed as WHERE: {:?}",
        source
    );
}

// ─── Analytic function with OVER clause ──────────────────────────────────

#[test]
fn analytic_over_clause_doesnt_confuse_depth() {
    let ctx = analyze(
        "SELECT ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY salary) AS rn, | FROM employees",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn plain_expression_parentheses_do_not_increase_query_depth() {
    let ctx = analyze("SELECT (salary + bonus) * | FROM employees");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn nested_function_parentheses_do_not_increase_query_depth() {
    let ctx = analyze("SELECT COALESCE(ROUND(salary, 2), 0) + | FROM employees");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn values_subquery_in_from_increases_query_depth() {
    let ctx = analyze("SELECT * FROM (VALUES (|)) v(c)");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn values_subquery_depth_returns_to_zero_after_close() {
    let ctx = analyze("SELECT * FROM (VALUES (1)) v(c) WHERE |");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn merge_using_subquery_increases_depth_inside_select_body() {
    let ctx = analyze("MERGE INTO target t USING (SELECT | FROM source) s ON (t.id = s.id)");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn merge_using_subquery_depth_returns_to_zero_after_close() {
    let ctx = analyze("MERGE INTO target t USING (SELECT id FROM source) s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = |");
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SetClause);
}

#[test]
fn merge_update_set_target_list_prefers_target_phase() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET |",
    );
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert_eq!(ctx.focused_tables, vec!["target".to_string()]);
}

#[test]
fn merge_update_set_after_comment_prefers_target_phase() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE /* keep merge action */ SET |",
    );
    assert_eq!(ctx.phase, SqlPhase::DmlSetTargetList);
    assert_eq!(ctx.focused_tables, vec!["target".to_string()]);
}

#[test]
fn merge_when_not_matched_insert_column_list_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (|) VALUES (s.id)",
    );
    assert_eq!(ctx.depth, 0);
    assert!(ctx.phase.is_column_context(), "phase: {:?}", ctx.phase);
}

#[test]
fn merge_when_not_matched_insert_after_comment_keeps_merge_column_list_phase() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN /* keep merge action */ INSERT (|) VALUES (s.id)",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::MergeInsertColumnList);
    assert_eq!(ctx.focused_tables, vec!["target".to_string()]);
}

#[test]
fn merge_when_not_matched_insert_values_is_values_clause() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (id) VALUES (|)",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn merge_when_matched_delete_where_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN DELETE WHERE |",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_update_then_delete_where_stays_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET t.val = s.val DELETE WHERE |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_insert_where_clause_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (id, val) VALUES (s.id, s.val) WHERE |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn phase_merge_insert_log_errors_into_is_table_context() {
    let ctx = analyze(
        "MERGE INTO tgt t USING src s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (id, val) VALUES (s.id, s.val) LOG ERRORS INTO |",
    );

    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn phase_merge_insert_log_errors_reject_limit_then_returning_is_set_clause() {
    let ctx = analyze(
        "MERGE INTO tgt t USING src s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (id, val) VALUES (s.id, s.val) \
         LOG ERRORS INTO err$_target REJECT LIMIT UNLIMITED RETURNING |",
    );

    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert!(ctx.phase.is_column_context());
    assert!(!ctx.phase.is_table_context());
}

#[test]
fn from_match_recognize_clause_preserves_match_keyword_for_phase_detection() {
    let ctx = analyze("SELECT * FROM sales MATCH RECOGNIZE (PARTITION BY |)");
    assert_eq!(ctx.phase, SqlPhase::MatchRecognizeClause);
    assert!(ctx.phase.is_column_context());

    let sales = ctx
        .tables_in_scope
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case("sales"));
    assert!(sales.is_some(), "tables: {:?}", ctx.tables_in_scope);
    assert!(
        sales
            .and_then(|table| table.alias.as_deref())
            .is_none_or(|alias| !alias.eq_ignore_ascii_case("MATCH")),
        "table alias must not be parsed as MATCH: {:?}",
        sales
    );
}

#[test]
fn lateral_values_subquery_in_from_increases_depth() {
    let ctx = analyze("SELECT * FROM base b CROSS APPLY (VALUES (|)) v(c)");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn from_subquery_with_update_body_increases_depth() {
    let ctx =
        analyze("SELECT * FROM (UPDATE employees SET salary = salary + 1 WHERE | RETURNING id) u");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn from_subquery_with_delete_body_increases_depth() {
    let ctx = analyze("SELECT * FROM (DELETE FROM employees WHERE | RETURNING id) d");
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn from_subquery_with_merge_body_increases_depth() {
    let ctx = analyze(
        "SELECT * FROM (MERGE INTO tgt t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.v = s.v WHERE |) m",
    );
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn merge_update_set_expression_does_not_start_new_update_target_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET update = |",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SetClause);
}

#[test]
fn merge_update_where_expression_does_not_start_new_update_target_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET val = 1 WHERE update = |",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn merge_insert_values_expression_does_not_start_new_insert_statement_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED THEN INSERT (id) VALUES (insert + |)",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn merge_delete_where_expression_does_not_start_new_delete_target_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN DELETE WHERE delete = |",
    );
    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn merge_when_matched_update_set_delete_where_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET t.val = s.val DELETE WHERE |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_when_matched_update_set_delete_where_keeps_source_table_in_scope() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET t.val = s.val DELETE WHERE s.| IS NOT NULL",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "TARGET"),
        "tables: {:?}",
        names
    );
    assert!(
        names.iter().any(|name| name == "SOURCE"),
        "tables: {:?}",
        names
    );
}

#[test]
fn merge_when_not_matched_by_source_delete_where_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED BY SOURCE THEN DELETE WHERE |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_when_not_matched_by_target_insert_values_is_values_clause() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED BY TARGET THEN INSERT (id) VALUES (|)",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::ValuesClause);
}

#[test]
fn merge_when_not_matched_by_source_update_set_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED BY SOURCE THEN UPDATE SET t.val = |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::SetClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn merge_when_not_matched_by_source_update_where_is_column_context() {
    let ctx = analyze(
        "MERGE INTO target t USING source s ON (t.id = s.id) \
         WHEN NOT MATCHED BY SOURCE THEN UPDATE SET t.val = 1 WHERE |",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn insert_first_else_into_is_table_context() {
    let ctx = analyze(
        "INSERT FIRST WHEN deptno = 10 THEN INTO emp10 (id) VALUES (id) ELSE INTO | SELECT id, deptno FROM emp",
    );

    assert_eq!(ctx.phase, SqlPhase::IntoClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn insert_first_else_into_does_not_capture_else_as_alias() {
    let ctx = analyze(
        "INSERT FIRST WHEN deptno = 10 THEN INTO emp10 (id) VALUES (id) ELSE INTO emp_other (id) VALUES (id) SELECT | FROM dual",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();
    assert!(
        aliases.iter().all(|alias| alias != "ELSE"),
        "ELSE keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

// ─── Complex CTE with multiple levels ────────────────────────────────────

#[test]
fn recursive_cte_keyword() {
    let ctx = analyze("WITH RECURSIVE tree AS (SELECT 1 AS id FROM dual) SELECT | FROM tree");
    let cte_n = cte_names(&ctx);
    assert!(cte_n.contains(&"TREE".to_string()), "CTEs: {:?}", cte_n);
}

#[test]
fn recursive_cte_search_by_is_column_context() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) SEARCH DEPTH FIRST BY | SET ord SELECT * FROM t",
    );
    assert_eq!(ctx.phase, SqlPhase::RecursiveCteColumnList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn recursive_cte_cycle_column_list_is_column_context() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) CYCLE | SET ord TO 1 DEFAULT 0 SELECT * FROM t",
    );
    assert_eq!(ctx.phase, SqlPhase::RecursiveCteColumnList);
    assert!(ctx.phase.is_column_context());
    assert_eq!(ctx.focused_tables, vec!["t".to_string()]);
}

#[test]
fn recursive_cte_cycle_set_uses_generated_column_name_phase() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) CYCLE n SET | TO 1 DEFAULT 0 SELECT * FROM t",
    );
    assert_eq!(ctx.phase, SqlPhase::RecursiveCteGeneratedColumnName);
    assert!(!ctx.phase.is_column_context());
}

#[test]
fn recursive_cte_search_keyword_is_not_parsed_as_alias_without_explicit_alias() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual) SEARCH DEPTH FIRST BY n SET ord SELECT t.| FROM t",
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();

    assert!(
        aliases.iter().all(|alias| alias != "SEARCH"),
        "SEARCH keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

#[test]
fn recursive_cte_cycle_keyword_is_not_parsed_as_alias_without_explicit_alias() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual) CYCLE n SET ord TO 1 DEFAULT 0 SELECT t.| FROM t",
    );

    let aliases: Vec<String> = ctx
        .tables_in_scope
        .iter()
        .filter_map(|table| table.alias.as_ref().map(|alias| alias.to_ascii_uppercase()))
        .collect();

    assert!(
        aliases.iter().all(|alias| alias != "CYCLE"),
        "CYCLE keyword must not be parsed as relation alias: {:?}",
        aliases
    );
}

#[test]
fn with_plsql_function_declaration_is_not_parsed_as_cte() {
    let ctx = analyze("WITH FUNCTION f RETURN NUMBER IS BEGIN RETURN 1; END; SELECT | FROM dual");

    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.is_empty(),
        "PL/SQL declaration should not create CTEs: {:?}",
        cte_n
    );

    let names = table_names(&ctx);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn with_plsql_procedure_declaration_is_not_parsed_as_cte() {
    let ctx = analyze("WITH PROCEDURE p IS BEGIN NULL; END; SELECT | FROM dual");

    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.is_empty(),
        "PL/SQL declaration should not create CTEs: {:?}",
        cte_n
    );

    let names = table_names(&ctx);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn with_plsql_package_declaration_is_not_parsed_as_cte() {
    let ctx = analyze(
        "WITH PACKAGE pkg_demo AS FUNCTION f RETURN NUMBER; END pkg_demo; SELECT | FROM dual",
    );

    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.is_empty(),
        "PL/SQL package declaration should not create CTEs: {:?}",
        cte_n
    );

    let names = table_names(&ctx);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn with_plsql_type_declaration_is_not_parsed_as_cte() {
    let ctx = analyze("WITH TYPE t_num IS TABLE OF NUMBER; SELECT | FROM dual");

    let cte_n = cte_names(&ctx);
    assert!(
        cte_n.is_empty(),
        "PL/SQL type declaration should not create CTEs: {:?}",
        cte_n
    );

    let names = table_names(&ctx);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn with_plsql_function_followed_by_cte_keeps_cte_visible() {
    let ctx = analyze(
        "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; \
         recursive_tree AS (SELECT 1 AS id FROM dual) \
         SELECT recursive_tree.| FROM recursive_tree",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "RECURSIVE_TREE"),
        "CTE after WITH FUNCTION should remain visible: {:?}",
        cte_names(&ctx)
    );
    let cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("recursive_tree"))
        .expect("expected recursive_tree CTE");
    assert_eq!(
        extract_select_list_columns(token_range_slice(
            ctx.statement_tokens.as_ref(),
            cte.body_range,
        )),
        vec!["id"]
    );
    assert_eq!(
        resolve_qualifier_tables("recursive_tree", &ctx.tables_in_scope),
        vec!["recursive_tree".to_string()]
    );
}

#[test]
fn with_plsql_function_with_nested_declare_block_keeps_following_cte_visible() {
    let ctx = analyze(
        r#"WITH FUNCTION calc_depth RETURN NUMBER IS
BEGIN
    DECLARE
        v_depth NUMBER := 1;
    BEGIN
        v_depth := v_depth + 1;
    END;
    RETURN v_depth;
END;
recursive_tree AS (SELECT 1 AS id FROM dual)
SELECT recursive_tree.| FROM recursive_tree"#,
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "RECURSIVE_TREE"),
        "CTE after WITH FUNCTION nested DECLARE block should remain visible: {:?}",
        cte_names(&ctx)
    );
    assert_eq!(
        resolve_qualifier_tables("recursive_tree", &ctx.tables_in_scope),
        vec!["recursive_tree".to_string()]
    );
}

#[test]
fn with_plsql_function_followed_by_explicit_with_query_keeps_cte_visible() {
    let ctx = analyze(
        "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; \
         WITH recursive_tree AS (SELECT 1 AS id FROM dual) \
         SELECT recursive_tree.| FROM recursive_tree",
    );

    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "RECURSIVE_TREE"),
        "explicit WITH after WITH FUNCTION should remain visible: {:?}",
        cte_names(&ctx)
    );
    assert_eq!(
        resolve_qualifier_tables("recursive_tree", &ctx.tables_in_scope),
        vec!["recursive_tree".to_string()]
    );
}

#[test]
fn with_plsql_function_followed_by_explicit_recursive_with_query_keeps_recursive_cte_visible() {
    let ctx = analyze(
        "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; \
         WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT r.| FROM r WHERE n < 3) \
         SELECT * FROM r",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "R"),
        "explicit recursive WITH after WITH FUNCTION should remain visible: {:?}",
        cte_names(&ctx)
    );
    let recursive_cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("r"))
        .expect("expected recursive CTE after explicit WITH");
    assert_eq!(recursive_cte.explicit_columns, vec!["n"]);
    assert_eq!(
        resolve_qualifier_tables("r", &ctx.tables_in_scope),
        vec!["r".to_string()]
    );
}

#[test]
fn with_plsql_function_followed_by_call_resets_with_phase() {
    let ctx = analyze(
        "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; CALL |pkg_demo.run_job()",
    );

    assert_eq!(ctx.depth, 0);
    assert_eq!(
        ctx.phase,
        SqlPhase::Initial,
        "CALL after WITH FUNCTION should leave declaration mode"
    );
    assert!(
        ctx.ctes.is_empty(),
        "CALL after WITH FUNCTION should not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn with_plsql_type_followed_by_table_query_resets_with_phase() {
    let ctx = analyze("WITH TYPE t_num IS TABLE OF NUMBER; TABLE |(t_num (1, 2, 3))");

    assert_eq!(ctx.depth, 0);
    assert_eq!(
        ctx.phase,
        SqlPhase::Initial,
        "TABLE query after WITH TYPE should leave declaration mode"
    );
    assert!(
        ctx.ctes.is_empty(),
        "TABLE query after WITH TYPE should not synthesize CTEs: {:?}",
        cte_names(&ctx)
    );
}

#[test]
fn nested_with_scalar_subquery_after_comment_still_starts_query_scope() {
    let ctx = analyze(
        "SELECT (/* scalar */ WITH cte AS (SELECT 1 AS id FROM dual) SELECT cte.| FROM cte) AS nested_id FROM dual",
    );

    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(
        has_name(&cte_names(&ctx), "CTE"),
        "ctes: {:?}",
        cte_names(&ctx)
    );
    let cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("cte"))
        .expect("nested CTE should be visible at cursor");
    assert_eq!(
        extract_select_list_columns(token_range_slice(
            ctx.statement_tokens.as_ref(),
            cte.body_range,
        )),
        vec!["id"]
    );
    assert_eq!(
        resolve_qualifier_tables("cte", &ctx.tables_in_scope),
        vec!["cte".to_string()]
    );
}

// ─── Oracle-specific: PIVOT/UNPIVOT ──────────────────────────────────────

#[test]
fn pivot_clause_phase() {
    let ctx = analyze("SELECT * FROM sales PIVOT (SUM(amount) FOR product IN ('A', 'B')) WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn pivot_clause_sum_argument_phase() {
    let ctx = analyze(
        "WITH s AS (SELECT DEPTNO, job, sal FROM oqt_t_emp) \
         SELECT * FROM s PIVOT (SUM(|) AS sum_sal FOR DEPTNO IN (10 AS D10))",
    );
    assert_eq!(ctx.phase, SqlPhase::PivotClause);
}

#[test]
fn pivot_clause_for_expression_phase() {
    let ctx = analyze(
        "WITH s AS (SELECT DEPTNO, job, sal FROM oqt_t_emp) \
         SELECT * FROM s PIVOT (SUM(sal) AS sum_sal FOR | IN (10 AS D10))",
    );
    assert_eq!(ctx.phase, SqlPhase::PivotClause);
}

#[test]
fn model_clause_dimension_by_phase_is_column_context() {
    let ctx = analyze(
        "WITH m AS ( \
           SELECT deptno, SUM(sal) AS sum_sal, COUNT(*) AS cnt \
           FROM oqt_t_emp \
           GROUP BY deptno \
         ) \
         SELECT deptno, sum_sal, cnt \
         FROM m \
         MODEL \
           DIMENSION BY (|) \
           MEASURES (sum_sal, cnt, 0 AS avg_sal_calc, 0 AS sum_plus_100) \
           RULES ( \
             avg_sal_calc[ANY] = ROUND(sum_sal[CV()] / NULLIF(cnt[CV()], 0), 2), \
             sum_plus_100[ANY] = sum_sal[CV()] + 100 \
           )",
    );
    assert_eq!(ctx.phase, SqlPhase::ModelClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn model_clause_rules_expression_phase_is_column_context() {
    let ctx = analyze(
        "WITH m AS ( \
           SELECT deptno, SUM(sal) AS sum_sal, COUNT(*) AS cnt \
           FROM oqt_t_emp \
           GROUP BY deptno \
         ) \
         SELECT deptno, sum_sal, cnt \
         FROM m \
         MODEL \
           DIMENSION BY (deptno) \
           MEASURES (sum_sal, cnt, 0 AS avg_sal_calc, 0 AS sum_plus_100) \
           RULES ( \
             avg_sal_calc[ANY] = ROUND(|[CV()] / NULLIF(cnt[CV()], 0), 2), \
             sum_plus_100[ANY] = sum_sal[CV()] + 100 \
           )",
    );
    assert_eq!(ctx.phase, SqlPhase::ModelClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn match_recognize_partition_by_phase_is_column_context() {
    let ctx = analyze(
        "SELECT * FROM oqt_t_emp \
         MATCH_RECOGNIZE (PARTITION BY | ORDER BY hiredate PATTERN (a b+) DEFINE b AS b.sal > PREV(b.sal))",
    );
    assert_eq!(ctx.phase, SqlPhase::MatchRecognizeClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn match_recognize_spaced_keywords_partition_by_phase_is_column_context() {
    let ctx = analyze(
        "SELECT * FROM oqt_t_emp \
         MATCH RECOGNIZE (PARTITION BY | ORDER BY hiredate PATTERN (a b+) DEFINE b AS b.sal > PREV(b.sal))",
    );
    assert_eq!(ctx.phase, SqlPhase::MatchRecognizeClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn match_recognize_pattern_variables_extracted() {
    let tokens = tokenize(
        "SELECT * FROM oqt_t_emp \
         MATCH_RECOGNIZE (PARTITION BY deptno ORDER BY hiredate PATTERN (a b+) DEFINE b AS b.sal > PREV(b.sal))",
    );
    let vars = extract_match_recognize_pattern_variables(&tokens);
    assert_eq!(vars, vec!["a", "b"]);
}

#[test]
fn match_recognize_pattern_variables_extracted_with_spaced_keywords() {
    let tokens = tokenize(
        "SELECT * FROM oqt_t_emp \
         MATCH RECOGNIZE (PARTITION BY deptno ORDER BY hiredate PATTERN (a b+) DEFINE b AS b.sal > PREV(b.sal))",
    );
    let vars = extract_match_recognize_pattern_variables(&tokens);
    assert_eq!(vars, vec!["a", "b"]);
}

#[test]
fn match_recognize_subset_variables_are_extracted() {
    let tokens = tokenize(
        "SELECT * FROM oqt_t_emp \
         MATCH_RECOGNIZE (\
            PARTITION BY deptno \
            ORDER BY hiredate \
            PATTERN (a b+) \
            SUBSET up = (a, b) \
            DEFINE b AS b.sal > PREV(b.sal)\
         )",
    );
    let vars = extract_match_recognize_pattern_variables(&tokens);
    assert_eq!(vars, vec!["a", "b", "up"]);
}

#[test]
fn match_recognize_multiple_subset_variables_are_extracted() {
    let tokens = tokenize(
        "SELECT * FROM oqt_t_emp \
         MATCH_RECOGNIZE (\
            ORDER BY hiredate \
            PATTERN (a b c) \
            SUBSET grp1 = (a, b), grp2 = (c) \
            DEFINE b AS b.sal > PREV(b.sal)\
         )",
    );
    let vars = extract_match_recognize_pattern_variables(&tokens);
    assert_eq!(vars, vec!["a", "b", "c", "grp1", "grp2"]);
}

#[test]
fn match_recognize_keyword_is_not_parsed_as_table_alias() {
    let ctx = analyze("SELECT * FROM oqt_t_emp MATCH_RECOGNIZE (PATTERN (a)) WHERE |");
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|t| t.alias.as_deref() != Some("MATCH_RECOGNIZE")),
        "MATCH_RECOGNIZE should not be parsed as table alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|t| (&t.name, &t.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn match_recognize_spaced_keywords_are_not_parsed_as_table_aliases() {
    let ctx = analyze("SELECT * FROM oqt_t_emp MATCH RECOGNIZE (PATTERN (a)) WHERE |");
    assert!(
        ctx.tables_in_scope
            .iter()
            .all(|t| t.alias.as_deref() != Some("MATCH") && t.alias.as_deref() != Some("RECOGNIZE")),
        "MATCH/RECOGNIZE should not be parsed as table alias: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|t| (&t.name, &t.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn match_recognize_clause_alias_after_base_table_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT mr.| FROM oqt_t_emp MATCH_RECOGNIZE (PARTITION BY deptno ORDER BY empno PATTERN (a) DEFINE a AS sal > 0) mr",
    );

    let alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("mr"))
    });
    assert!(
        alias.is_some(),
        "match_recognize alias mr should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("mr", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "match_recognize alias should resolve for qualifiers, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn match_recognize_clause_alias_after_base_table_registers_virtual_relation_body() {
    let ctx = analyze(
        "SELECT mr.| \
         FROM oqt_t_emp \
         MATCH_RECOGNIZE ( \
           MEASURES FIRST(ename) AS start_name \
           PATTERN (a) \
           DEFINE a AS sal > 0 \
         ) mr",
    );

    let virtual_relation = ctx
        .subqueries
        .iter()
        .find(|subq| subq.alias.eq_ignore_ascii_case("mr"))
        .expect("expected MATCH_RECOGNIZE alias mr to be tracked as a virtual relation");
    let body_tokens = token_range_slice(ctx.statement_tokens.as_ref(), virtual_relation.body_range);
    let generated = extract_match_recognize_generated_columns(body_tokens);

    assert!(
        generated
            .iter()
            .any(|column| column.eq_ignore_ascii_case("start_name")),
        "expected MATCH_RECOGNIZE MEASURES alias in virtual relation body, got: {:?}",
        generated
    );
    assert!(
        generated
            .iter()
            .any(|column| column.eq_ignore_ascii_case("a")),
        "expected MATCH_RECOGNIZE pattern variable in virtual relation body, got: {:?}",
        generated
    );
}

#[test]
fn match_recognize_spaced_clause_alias_after_base_table_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT mrs.| FROM oqt_t_emp MATCH RECOGNIZE (PARTITION BY deptno ORDER BY empno PATTERN (a) DEFINE a AS sal > 0) mrs",
    );

    let alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("mrs"))
    });
    assert!(
        alias.is_some(),
        "spaced MATCH RECOGNIZE alias mrs should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("mrs", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "spaced MATCH RECOGNIZE alias should resolve for qualifiers, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pivot_clause_alias_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT p.| FROM (SELECT deptno, job, sal FROM oqt_t_emp) PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) p",
    );

    let pivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("p"))
    });
    assert!(
        pivot_alias.is_some(),
        "pivot clause alias p should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("p", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "pivot clause alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unpivot_clause_alias_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT u.| FROM (SELECT deptno, sal FROM oqt_t_emp) UNPIVOT (amount FOR metric IN (sal AS 'SAL')) u",
    );

    let unpivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("u"))
    });
    assert!(
        unpivot_alias.is_some(),
        "unpivot clause alias u should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("u", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "unpivot clause alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pivot_clause_alias_after_base_table_is_collected_for_qualifier_resolution() {
    let ctx =
        analyze("SELECT p.| FROM oqt_t_emp PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) p");

    let pivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("p"))
    });
    assert!(
        pivot_alias.is_some(),
        "pivot clause alias p should be present for base table source, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("p", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "pivot base-table alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pivot_clause_alias_after_subquery_keeps_pivot_projection_in_virtual_relation_body() {
    let ctx = analyze(
        "SELECT p.| \
         FROM (SELECT deptno, job, sal FROM oqt_t_emp) \
         PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) p",
    );

    let virtual_relation = ctx
        .subqueries
        .iter()
        .find(|subq| subq.alias.eq_ignore_ascii_case("p"))
        .expect("expected PIVOT alias p to be tracked as a virtual relation");
    let body_tokens = token_range_slice(ctx.statement_tokens.as_ref(), virtual_relation.body_range);
    let projected = extract_oracle_pivot_unpivot_projection_columns(body_tokens);

    assert!(
        projected
            .iter()
            .any(|column| column.eq_ignore_ascii_case("clerk_sal")),
        "expected PIVOT generated alias in virtual relation body, got: {:?}",
        projected
    );
}

#[test]
fn pivot_xml_clause_alias_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT px.| FROM (SELECT deptno, job, sal FROM oqt_t_emp) PIVOT XML (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) px",
    );

    let pivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("px"))
    });
    assert!(
        pivot_alias.is_some(),
        "pivot xml alias px should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("px", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "pivot xml alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unpivot_include_nulls_clause_alias_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT un.| FROM (SELECT deptno, sal, bonus FROM oqt_t_emp) UNPIVOT INCLUDE NULLS (amount FOR metric IN (sal AS 'SAL', bonus AS 'BONUS')) un",
    );

    let unpivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("un"))
    });
    assert!(
        unpivot_alias.is_some(),
        "unpivot include nulls alias un should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("un", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "unpivot include nulls alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unpivot_exclude_nulls_clause_alias_after_base_table_is_collected() {
    let ctx = analyze(
        "SELECT ux.| FROM oqt_t_emp UNPIVOT EXCLUDE NULLS (amount FOR metric IN (sal AS 'SAL')) ux",
    );

    let unpivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("ux"))
    });
    assert!(
        unpivot_alias.is_some(),
        "unpivot exclude nulls alias ux should be present for base table source, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("ux", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "unpivot exclude nulls base-table alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn model_clause_alias_after_base_table_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT md.| FROM oqt_t_emp MODEL DIMENSION BY (deptno) MEASURES (sal) RULES (sal[deptno] = sal[deptno]) md",
    );

    let model_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("md"))
    });
    assert!(
        model_alias.is_some(),
        "model clause alias md should be present for base table source, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("md", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "model base-table alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn model_clause_alias_after_subquery_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT ms.| FROM (SELECT deptno, sal FROM oqt_t_emp) MODEL DIMENSION BY (deptno) MEASURES (sal) RULES (sal[deptno] = sal[deptno]) ms",
    );

    let model_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("ms"))
    });
    assert!(
        model_alias.is_some(),
        "model clause alias ms should be present for subquery source, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("ms", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "model subquery alias should be collected, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pivot_clause_alias_with_as_keyword_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT pa.| FROM oqt_t_emp PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) AS pa",
    );

    let pivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("pa"))
    });
    assert!(
        pivot_alias.is_some(),
        "pivot clause alias pa with AS should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("pa", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "pivot alias with AS should resolve for qualifiers, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pivot_clause_source_followed_by_comma_relation_collects_next_table() {
    let ctx = analyze(
        "SELECT d.| FROM oqt_t_emp PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) p, dept d WHERE p.clerk_sal > 0",
    );

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "DEPT"),
        "table after pivot source should remain in scope: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope.iter().any(|table| table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("d"))),
        "alias for relation after pivot source should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn model_clause_source_followed_by_comma_relation_collects_next_table() {
    let ctx = analyze(
        "SELECT d.| FROM oqt_t_emp MODEL DIMENSION BY (deptno) MEASURES (sal) RULES (sal[deptno] = sal[deptno]) md, dept d WHERE md.sal > 0",
    );

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "DEPT"),
        "table after model source should remain in scope: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope.iter().any(|table| table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("d"))),
        "alias for relation after model source should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn match_recognize_source_followed_by_comma_relation_collects_next_table() {
    let ctx = analyze(
        "SELECT d.| FROM oqt_t_emp MATCH_RECOGNIZE (PATTERN (a) DEFINE a AS sal > 0) mr, dept d WHERE mr.a IS NOT NULL",
    );

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "DEPT"),
        "table after match_recognize source should remain in scope: {:?}",
        names
    );
    assert!(
        ctx.tables_in_scope.iter().any(|table| table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("d"))),
        "alias for relation after match_recognize source should be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unpivot_clause_alias_with_as_keyword_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT ua.| FROM oqt_t_emp UNPIVOT EXCLUDE NULLS (amount FOR metric IN (sal AS 'SAL')) AS ua",
    );

    let unpivot_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("ua"))
    });
    assert!(
        unpivot_alias.is_some(),
        "unpivot clause alias ua with AS should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("ua", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "unpivot alias with AS should resolve for qualifiers, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn model_clause_rules_iterate_alias_is_collected_for_qualifier_resolution() {
    let ctx = analyze(
        "SELECT mi.| FROM oqt_t_emp MODEL DIMENSION BY (deptno) MEASURES (sal) RULES ITERATE (2) (sal[deptno] = sal[deptno]) mi",
    );

    let model_alias = ctx.tables_in_scope.iter().find(|table| {
        table
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case("mi"))
    });
    assert!(
        model_alias.is_some(),
        "model iterate alias mi should be present, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );

    let resolved = resolve_qualifier_tables("mi", &ctx.tables_in_scope);
    assert!(
        !resolved.is_empty(),
        "model iterate alias should resolve for qualifiers, tables: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn json_table_arguments_can_resolve_left_relation_alias() {
    let ctx = analyze(
        "SELECT * \
         FROM oqt_t_json j \
         CROSS JOIN JSON_TABLE(j.|, '$' COLUMNS (order_id NUMBER PATH '$.order_id')) jt",
    );
    let resolved = resolve_qualifier_tables("j", &ctx.tables_in_scope);
    assert!(
        resolved
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_json")),
        "json_table argument should resolve left table alias: {:?}",
        resolved
    );
}

#[test]
fn extract_table_function_columns_includes_nested_columns_clause() {
    let ctx = analyze(
        "SELECT jt.| \
         FROM oqt_t_json j \
         CROSS JOIN JSON_TABLE( \
           j.payload, \
           '$' \
           COLUMNS ( \
             order_id NUMBER PATH '$.order_id', \
             NESTED PATH '$.items[*]' \
             COLUMNS ( \
               sku VARCHAR2(30) PATH '$.sku', \
               qty NUMBER PATH '$.qty', \
               price NUMBER PATH '$.price' \
             ) \
           ) \
         ) jt",
    );

    let subq = ctx
        .subqueries
        .iter()
        .find(|s| s.alias.eq_ignore_ascii_case("jt"))
        .expect("expected json_table alias jt");
    let cols = extract_table_function_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        subq.body_range,
    ));

    for expected in ["order_id", "sku", "qty", "price"] {
        assert!(
            cols.iter().any(|c| c.eq_ignore_ascii_case(expected)),
            "expected nested json_table column `{expected}` in {:?}",
            cols
        );
    }
}

#[test]
fn xmltable_arguments_can_resolve_left_relation_alias() {
    let ctx = analyze(
        "SELECT * \
         FROM oqt_t_xml x \
         CROSS JOIN XMLTABLE('/rows/row' PASSING x.| COLUMNS id NUMBER PATH '@id') xt",
    );
    let resolved = resolve_qualifier_tables("x", &ctx.tables_in_scope);
    assert!(
        resolved
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_xml")),
        "xmltable argument should resolve left table alias: {:?}",
        resolved
    );
}

#[test]
fn unqualified_json_table_keeps_left_relation_scope() {
    let ctx = analyze(
        "SELECT * \
         FROM oqt_t_json j \
         CROSS JOIN json_table(j.|, '$' COLUMNS (id NUMBER PATH '$.id')) jt",
    );
    let resolved = resolve_qualifier_tables("j", &ctx.tables_in_scope);
    assert!(
        resolved
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_json")),
        "lowercase json_table should keep left table alias visible: {:?}",
        resolved
    );
}

#[test]
fn schema_qualified_json_table_keeps_left_relation_scope() {
    let ctx = analyze(
        "SELECT * \
         FROM oqt_t_json j \
         CROSS JOIN SYS.JSON_TABLE(j.|, '$' COLUMNS (id NUMBER PATH '$.id')) jt",
    );
    let resolved = resolve_qualifier_tables("j", &ctx.tables_in_scope);
    assert!(
        resolved
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_json")),
        "schema-qualified json_table should keep left table alias visible: {:?}",
        resolved
    );
}

#[test]
fn dblink_json_table_keeps_left_relation_scope() {
    let ctx = analyze(
        "SELECT * \
         FROM oqt_t_json j \
         CROSS JOIN JSON_TABLE@REMDB(j.|, '$' COLUMNS (id NUMBER PATH '$.id')) jt",
    );
    let resolved = resolve_qualifier_tables("j", &ctx.tables_in_scope);
    assert!(
        resolved
            .iter()
            .any(|name| name.eq_ignore_ascii_case("oqt_t_json")),
        "dblink json_table should keep left table alias visible: {:?}",
        resolved
    );
}

#[test]
fn extract_select_list_leading_qualifiers_reads_incomplete_references() {
    let tokens = tokenize("SELECT jt., jt., jt. FROM dual");
    let qualifiers = extract_select_list_leading_qualifiers(&tokens);
    assert_eq!(qualifiers, vec!["jt"]);
}

#[test]
fn extract_oracle_pivot_projection_columns_from_subquery_star_select() {
    let tokens = tokenize(
        "SELECT * FROM (SELECT DEPTNO, job, SAL FROM oqt_t_emp) \
         PIVOT (SUM(SAL) FOR DEPTNO IN (10 AS D10, 20 AS D20, 30 AS D30))",
    );
    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["job", "D10", "D20", "D30"]);
}

#[test]
fn extract_oracle_pivot_projection_columns_from_string_literals_without_alias() {
    let tokens = tokenize("SELECT * FROM sales PIVOT (SUM(amount) FOR product IN ('A', 'B', 'C'))");

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["A", "B", "C"]);
}

#[test]
fn extract_oracle_pivot_projection_columns_from_numeric_literals_without_alias() {
    let tokens = tokenize("SELECT * FROM sales PIVOT (SUM(amount) FOR quarter_id IN (1, 2, 3))");

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["1", "2", "3"]);
}

#[test]
fn extract_oracle_pivot_projection_columns_unescapes_string_literals_without_alias() {
    let tokens = tokenize("SELECT * FROM sales PIVOT (SUM(amount) FOR product IN ('O''CLOCK'))");

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["O'CLOCK"]);
}

#[test]
fn extract_oracle_pivot_xml_projection_keeps_source_columns_without_generated_aliases() {
    let tokens = tokenize(
        "SELECT * FROM (SELECT DEPTNO, job, SAL FROM oqt_t_emp) \
         PIVOT XML (SUM(SAL) FOR DEPTNO IN (ANY))",
    );
    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["job"]);
}

#[test]
fn extract_oracle_unpivot_generated_columns_from_clause() {
    let tokens = tokenize(
        "SELECT * FROM p \
         UNPIVOT (sum_sal FOR dept_tag IN (D10 AS '10', D20 AS '20', D30 AS '30'))",
    );
    let cols = extract_oracle_unpivot_generated_columns(&tokens);
    assert_eq!(cols, vec!["sum_sal", "dept_tag"]);
}

#[test]
fn extract_oracle_model_generated_columns_from_measures_clause() {
    let tokens = tokenize(
        "SELECT deptno, sum_sal \
         FROM m \
         MODEL \
           DIMENSION BY (deptno) \
           MEASURES (sum_sal, cnt, 0 AS avg_sal_calc, 0 AS sum_plus_100) \
           RULES ( \
             avg_sal_calc[ANY] = ROUND(sum_sal[CV()] / NULLIF(cnt[CV()], 0), 2), \
             sum_plus_100[ANY] = sum_sal[CV()] + 100 \
           )",
    );
    let cols = extract_oracle_model_generated_columns(&tokens);
    assert_eq!(cols, vec!["sum_sal", "cnt", "avg_sal_calc", "sum_plus_100"]);
}

#[test]
fn extract_recursive_cte_generated_columns_from_search_and_cycle_clauses() {
    let ctx = analyze(
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) \
         SEARCH DEPTH FIRST BY n SET ord_seq \
         CYCLE n SET is_cycle TO 'Y' DEFAULT 'N' \
         SELECT |* FROM t",
    );

    let cte = ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("t"))
        .expect("expected recursive CTE t");

    let cols =
        extract_recursive_cte_generated_columns(ctx.statement_tokens.as_ref(), cte.body_range.end);
    assert_eq!(cols, vec!["ord_seq", "is_cycle"]);
}

#[test]
fn extract_match_recognize_generated_columns_from_measures_and_pattern() {
    let tokens = tokenize(
        "SELECT * FROM emp MATCH_RECOGNIZE ( \
            MEASURES FIRST(ename) AS start_name, LAST(ename) AS end_name \
            PATTERN (a b+) \
            DEFINE b AS b.sal > PREV(b.sal) \
        )",
    );

    let cols = extract_match_recognize_generated_columns(&tokens);
    assert_eq!(cols, vec!["start_name", "end_name", "a", "b"]);
}

#[test]
fn extract_match_recognize_generated_columns_accepts_measures_alias_without_as() {
    let tokens = tokenize(
        "SELECT * FROM emp MATCH_RECOGNIZE ( \
            MEASURES FIRST(ename) start_name, LAST(ename) end_name \
            PATTERN (a b+) \
            DEFINE b AS b.sal > PREV(b.sal) \
        )",
    );

    let cols = extract_match_recognize_generated_columns(&tokens);
    assert_eq!(cols, vec!["start_name", "end_name", "a", "b"]);
}

#[test]
fn extract_match_recognize_generated_columns_keeps_pattern_variables_with_subset_after_after_match_skip(
) {
    let tokens = tokenize(
        "SELECT * FROM emp MATCH_RECOGNIZE ( \
            PATTERN (a b c) \
            AFTER MATCH SKIP TO LAST b \
            SUBSET grp1 = (a, b), grp2 = (c) \
            DEFINE b AS b.sal > PREV(b.sal), c AS c.sal >= b.sal \
        )",
    );

    let cols = extract_match_recognize_generated_columns(&tokens);
    assert_eq!(cols, vec!["a", "b", "c", "grp1", "grp2"]);
}

#[test]
fn infer_source_columns_uses_match_recognize_generated_columns_when_select_list_is_star() {
    let tokens = tokenize(
        "SELECT * FROM ( \
            SELECT * FROM emp MATCH_RECOGNIZE ( \
                MEASURES FIRST(ename) AS start_name, LAST(ename) AS end_name \
                PATTERN (a b+) \
                DEFINE b AS b.sal > PREV(b.sal) \
            ) mr \
        ) q",
    );

    let cols = infer_source_columns_before_clause(&tokens, tokens.len());
    assert!(
        cols.iter()
            .any(|col| col.eq_ignore_ascii_case("start_name")),
        "expected MATCH_RECOGNIZE MEASURES alias to be inferred, got: {:?}",
        cols
    );
    assert!(
        cols.iter().any(|col| col.eq_ignore_ascii_case("end_name")),
        "expected MATCH_RECOGNIZE MEASURES alias to be inferred, got: {:?}",
        cols
    );
}

#[test]
fn extract_match_recognize_generated_columns_ignores_nested_recognize_keyword_without_top_level_clause(
) {
    let tokens = tokenize(
        "SELECT * FROM ( \
            SELECT MATCH(1) AS match_token, RECOGNIZE(1) AS recognize_token FROM dual \
         ) src",
    );

    let cols = extract_match_recognize_generated_columns(&tokens);
    assert!(
        cols.is_empty(),
        "non-MATCH_RECOGNIZE SQL should not synthesize MATCH_RECOGNIZE columns: {:?}",
        cols
    );
}

#[test]
fn extract_match_recognize_generated_columns_accepts_comment_split_keyword_pair() {
    let tokens = tokenize(
        "SELECT * FROM emp MATCH /* split */ RECOGNIZE ( \
            MEASURES FIRST(ename) AS first_name \
            PATTERN (a+) \
            DEFINE a AS a.sal > 0 \
        )",
    );

    let cols = extract_match_recognize_generated_columns(&tokens);
    assert_eq!(cols, vec!["first_name", "a"]);
}

#[test]
fn extract_oracle_unpivot_projection_with_nested_pivot_source() {
    let tokens = tokenize(
        "SELECT * FROM ( \
            SELECT * FROM (SELECT DEPTNO, job, SAL FROM oqt_t_emp) \
            PIVOT (SUM(SAL) FOR DEPTNO IN (10 AS D10, 20 AS D20, 30 AS D30)) \
         ) \
         UNPIVOT (sum_sal FOR dept_tag IN (D10 AS '10', D20 AS '20', D30 AS '30'))",
    );
    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["job", "sum_sal", "dept_tag"]);
}

#[test]
fn extract_oracle_pivot_projection_with_multi_column_for_clause() {
    let tokens = tokenize(
        "SELECT * FROM (SELECT year_key, quarter_key, sales_amt FROM sales_fact) \
         PIVOT (SUM(sales_amt) FOR (year_key, quarter_key) IN ((2024, 'Q1') AS y2024_q1))",
    );

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["y2024_q1"]);
}

#[test]
fn extract_oracle_pivot_projection_with_quoted_generated_alias() {
    let tokens = tokenize(
        "SELECT * FROM sales_fact \
         PIVOT (SUM(sales_amt) FOR quarter_key IN ('Q1' AS \"Q1 SALES\"))",
    );

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(cols, vec!["Q1 SALES"]);
}

#[test]
fn extract_oracle_unpivot_projection_with_multi_column_output_and_for_clause() {
    let tokens = tokenize(
        "SELECT * FROM sales_half \
         UNPIVOT ((sales_amt, cost_amt) FOR (half_year, quarter_tag) \
         IN ((h1_sales, h1_cost) AS ('H1', 'QX'), (h2_sales, h2_cost) AS ('H2', 'QY')))",
    );

    let cols = extract_oracle_pivot_unpivot_projection_columns(&tokens);
    assert_eq!(
        cols,
        vec!["sales_amt", "cost_amt", "half_year", "quarter_tag"]
    );
}

#[test]
fn extract_oracle_unpivot_generated_columns_strip_quotes() {
    let tokens = tokenize(
        "SELECT * FROM sales_half \
         UNPIVOT ((\"sales amount\") FOR \"quarter tag\" IN (h1_sales AS 'H1'))",
    );

    let cols = extract_oracle_unpivot_generated_columns(&tokens);
    assert_eq!(cols, vec!["sales amount", "quarter tag"]);
}

// ─── SELECT list column extraction tests ─────────────────────────────────

#[test]
fn extract_simple_columns() {
    let tokens = tokenize("SELECT id, name, age FROM users");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["id", "name", "age"]);
}

#[test]
fn extract_qualified_columns() {
    let tokens = tokenize("SELECT e.empno, e.ename FROM emp e");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["empno", "ename"]);
}

#[test]
fn extract_aliased_columns() {
    let tokens = tokenize("SELECT COUNT(*) AS cnt, AVG(sal) AS avg_sal FROM emp");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["cnt", "avg_sal"]);
}

#[test]
fn extract_implicit_alias() {
    let tokens = tokenize("SELECT e.deptno, COUNT(*) emp_cnt FROM emp e GROUP BY e.deptno");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["deptno", "emp_cnt"]);
}

#[test]
fn extract_star_skipped() {
    let tokens = tokenize("SELECT * FROM emp");
    let cols = extract_select_list_columns(&tokens);
    assert!(cols.is_empty());
}

#[test]
fn extract_qualified_star_skipped() {
    let tokens = tokenize("SELECT e.* FROM emp e");
    let cols = extract_select_list_columns(&tokens);
    assert!(cols.is_empty());
}

#[test]
fn extract_mixed_columns_and_star() {
    let tokens = tokenize("SELECT id, e.*, name FROM emp e");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["id", "name"]);
}

#[test]
fn extract_wildcard_tables_unqualified_star() {
    let ctx = analyze("SELECT | FROM help");
    let tokens = tokenize("SELECT * FROM help");
    let tables = extract_select_list_wildcard_tables(&tokens, &ctx.tables_in_scope);
    let upper: Vec<String> = tables.into_iter().map(|t| t.to_uppercase()).collect();
    assert_eq!(upper, vec!["HELP"]);
}

#[test]
fn extract_wildcard_tables_qualified_star() {
    let ctx = analyze("SELECT | FROM help h");
    let tokens = tokenize("SELECT h.* FROM help h");
    let tables = extract_select_list_wildcard_tables(&tokens, &ctx.tables_in_scope);
    let upper: Vec<String> = tables.into_iter().map(|t| t.to_uppercase()).collect();
    assert_eq!(upper, vec!["HELP"]);
}

#[test]
fn extract_wildcard_tables_multiple_sources() {
    let ctx = analyze("SELECT | FROM help h JOIN dept d ON d.id = h.id");
    let tokens = tokenize("SELECT h.*, d.* FROM help h JOIN dept d ON d.id = h.id");
    let tables = extract_select_list_wildcard_tables(&tokens, &ctx.tables_in_scope);
    let upper: Vec<String> = tables.into_iter().map(|t| t.to_uppercase()).collect();
    assert_eq!(upper, vec!["HELP", "DEPT"]);
}

#[test]
fn extract_select_distinct() {
    let tokens = tokenize("SELECT DISTINCT id, name FROM users");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["id", "name"]);
}

#[test]
fn extract_nested_function_with_alias() {
    let tokens = tokenize("SELECT NVL(COALESCE(a, b), c) AS result FROM t");
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["result"]);
}

#[test]
fn extract_scalar_subquery_with_alias() {
    let tokens = tokenize(
        "SELECT \
           oh.order_id, \
           (SELECT SUM(oi.qty*oi.unit_price) \
            FROM oqt_t_order_item oi \
            WHERE oi.order_id = oh.order_id) AS amt \
         FROM oqt_t_order_hdr oh",
    );
    let cols = extract_select_list_columns(&tokens);
    assert_eq!(cols, vec!["order_id", "amt"]);
}

#[test]
fn extract_table_function_columns_from_xmltable_columns_clause() {
    let tokens = tokenize(
        "'/root/dept' PASSING t.payload COLUMNS \
         deptno NUMBER PATH '@deptno', \
         name VARCHAR2(30) PATH 'name/text()', \
         loc VARCHAR2(30) PATH 'loc/text()'",
    );
    let cols = extract_table_function_columns(&tokens);
    assert_eq!(cols, vec!["deptno", "name", "loc"]);
}

#[test]
fn extract_table_function_columns_skips_select_subquery_bodies() {
    let tokens =
        tokenize("SELECT id, XMLTABLE('/x' PASSING t.payload COLUMNS c NUMBER PATH '/x') FROM t");
    let cols = extract_table_function_columns(&tokens);
    assert!(cols.is_empty());
}

// ─── CTE body token capture tests ───────────────────────────────────────

#[test]
fn cte_body_tokens_captured() {
    let ctx = analyze(
        "WITH emp_base AS (SELECT e.empno, e.ename, e.deptno FROM emp e) \
         SELECT eb.| FROM emp_base eb",
    );
    let cte = ctx
        .ctes
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("emp_base"))
        .unwrap();
    assert!(!cte.body_range.is_empty());
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        cte.body_range,
    ));
    assert_eq!(cols, vec!["empno", "ename", "deptno"]);
}

#[test]
fn cte_explicit_columns_present() {
    let ctx = analyze("WITH cte(x, y) AS (SELECT id, name FROM users) SELECT c.| FROM cte c");
    let cte = ctx
        .ctes
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("cte"))
        .unwrap();
    assert_eq!(cte.explicit_columns.len(), 2);
    assert_eq!(cte.explicit_columns, vec!["x", "y"]);
}

#[test]
fn cte_chain_columns_inferred() {
    let sql = "WITH emp_base AS (\
            SELECT e.empno, e.ename, e.deptno, e.sal, e.hiredate FROM emp e\
        ), \
        dept_agg AS (\
            SELECT eb.deptno, COUNT(*) AS emp_cnt, AVG(eb.sal) AS avg_sal \
            FROM emp_base eb GROUP BY eb.deptno\
        ) \
        SELECT d.deptno, c.| FROM dept d JOIN dept_agg c ON c.deptno = d.deptno";
    let ctx = analyze(sql);
    let dept_agg = ctx
        .ctes
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("dept_agg"))
        .unwrap();
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        dept_agg.body_range,
    ));
    assert_eq!(cols, vec!["deptno", "emp_cnt", "avg_sal"]);
}

#[test]
fn cte_emp_base_columns_inferred() {
    let sql = "WITH emp_base AS (\
            SELECT e.empno, e.ename, e.deptno, e.sal, e.hiredate FROM emp e\
        ), \
        dept_agg AS (\
            SELECT eb.deptno, COUNT(*) AS emp_cnt, AVG(eb.sal) AS avg_sal \
            FROM emp_base eb GROUP BY eb.deptno\
        ) \
        SELECT d.deptno, c.avg_sal FROM dept d JOIN dept_agg c ON c.deptno = d.deptno WHERE |";
    let ctx = analyze(sql);
    let emp_base = ctx
        .ctes
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("emp_base"))
        .unwrap();
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        emp_base.body_range,
    ));
    assert_eq!(cols, vec!["empno", "ename", "deptno", "sal", "hiredate"]);
}

// ─── Subquery alias column extraction tests ─────────────────────────────

#[test]
fn subquery_alias_columns_captured() {
    let ctx = analyze("SELECT sub.| FROM (SELECT id, name, age FROM users) sub");
    assert!(!ctx.subqueries.is_empty());
    let subq = ctx
        .subqueries
        .iter()
        .find(|s| s.alias.eq_ignore_ascii_case("sub"))
        .unwrap();
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        subq.body_range,
    ));
    assert_eq!(cols, vec!["id", "name", "age"]);
}

#[test]
fn subquery_without_alias_columns_captured() {
    let ctx = analyze("SELECT | FROM (SELECT id, name FROM users)");
    assert_eq!(ctx.subqueries.len(), 1);
    let subq = &ctx.subqueries[0];
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        subq.body_range,
    ));
    assert_eq!(cols, vec!["id", "name"]);
}

#[test]
fn subquery_alias_with_expressions() {
    let ctx = analyze(
        "SELECT v.| FROM (SELECT dept_id, COUNT(*) AS cnt, MAX(sal) max_sal FROM emp GROUP BY dept_id) v",
    );
    let subq = ctx
        .subqueries
        .iter()
        .find(|s| s.alias.eq_ignore_ascii_case("v"))
        .unwrap();
    let cols = extract_select_list_columns(token_range_slice(
        ctx.statement_tokens.as_ref(),
        subq.body_range,
    ));
    assert_eq!(cols, vec!["dept_id", "cnt", "max_sal"]);
}

#[test]
fn malformed_subquery_parentheses_do_not_panic() {
    let ctx = analyze("SELECT * FROM (SELECT * FROM emp)) broken_alias |");
    let names = table_names(&ctx);
    assert!(names.contains(&"BROKEN_ALIAS".to_string()));
}

// ─── EXTRACT / TRIM function-internal FROM ───────────────────────────────

#[test]
fn extract_from_does_not_trigger_from_clause() {
    // EXTRACT(YEAR FROM ...) uses FROM as function syntax, not as a SQL clause.
    // The cursor inside EXTRACT should stay in column context (SelectList).
    let ctx = analyze("SELECT EXTRACT(YEAR FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn extract_with_comment_before_open_paren_keeps_function_from_context() {
    let ctx = analyze("SELECT EXTRACT /*inline*/ (YEAR FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn trim_from_does_not_trigger_from_clause() {
    // TRIM(LEADING '0' FROM col) uses FROM as function syntax.
    let ctx = analyze("SELECT TRIM(LEADING '0' FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn xmlcast_from_inside_function_recognizes_real_from_clause() {
    let ctx = analyze("SELECT XMLCAST(doc AS CLOB FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn xmlcast_from_inside_function_keeps_table_scope_after_where() {
    let ctx = analyze("SELECT XMLCAST(doc AS CLOB FROM employees) FROM dual WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"DUAL".to_string()), "tables: {:?}", names);
}

#[test]
fn substring_from_does_not_trigger_from_clause() {
    // SUBSTRING(col FROM start FOR count) uses FROM as function syntax.
    let ctx = analyze("SELECT SUBSTRING(name FROM | FOR 2) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn overlay_from_does_not_trigger_from_clause() {
    // OVERLAY(col PLACING txt FROM start FOR count) also consumes FROM internally.
    let ctx = analyze("SELECT OVERLAY(name PLACING 'X' FROM | FOR 1) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn position_from_does_not_trigger_from_clause() {
    // POSITION(sub IN source) uses IN, but POSITION(sub FROM source) is allowed in some dialects
    // and should not be interpreted as a SQL FROM clause.
    let ctx = analyze("SELECT POSITION('a' FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn normalize_from_does_not_trigger_from_clause() {
    // PostgreSQL NORMALIZE(value FROM form) consumes FROM inside function syntax.
    let ctx = analyze("SELECT NORMALIZE(name FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn trim_array_from_does_not_trigger_from_clause() {
    // PostgreSQL TRIM_ARRAY(array FROM n) uses FROM as a function keyword.
    let ctx = analyze("SELECT TRIM_ARRAY(items FROM |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn real_from_after_position_still_works() {
    let ctx = analyze("SELECT POSITION('a' FROM name) FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn real_from_after_extract_still_works() {
    // The outer FROM clause should still be detected correctly.
    let ctx = analyze("SELECT EXTRACT(YEAR FROM hire_date) FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn malformed_trim_missing_close_paren_recovers_real_from_clause() {
    // Recovery case: if TRIM's closing ')' is missing, the parser should still
    // treat the next FROM as a real SQL clause instead of swallowing it as an
    // endless function-internal FROM.
    let ctx = analyze("SELECT TRIM(LEADING '0' FROM name FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert!(ctx.phase.is_table_context());
}

#[test]
fn malformed_trim_missing_close_paren_still_collects_from_tables() {
    let ctx = analyze("SELECT TRIM(LEADING '0' FROM name FROM employees WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn subquery_from_inside_parens_still_works() {
    // A subquery inside parentheses should still detect FROM correctly.
    let ctx = analyze("SELECT * FROM (SELECT id FROM |");
    assert_eq!(ctx.phase, SqlPhase::FromClause);
    assert_eq!(ctx.depth, 1);
}

#[test]
fn extract_does_not_confuse_table_collection() {
    // Tables referenced after EXTRACT should still be collected.
    let ctx = analyze("SELECT EXTRACT(YEAR FROM hire_date) FROM employees WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

// ─── DELETE without FROM ─────────────────────────────────────────────────

#[test]
fn delete_without_from_collects_target_table() {
    // Oracle allows DELETE table_name WHERE ... (without FROM).
    let ctx = analyze("DELETE employees WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "DELETE without FROM should collect target table: {:?}",
        names
    );
}

#[test]
fn delete_with_from_collects_target_table() {
    // Standard DELETE FROM table_name should also work.
    let ctx = analyze("DELETE FROM employees WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(
        names.contains(&"EMPLOYEES".to_string()),
        "tables: {:?}",
        names
    );
}

#[test]
fn delete_from_returning_has_focused_target_table() {
    // DELETE FROM t RETURNING should focus on the target table for column suggestions.
    let ctx = analyze("DELETE FROM employees WHERE id > 0 RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert_eq!(
        ctx.focused_tables,
        vec!["employees".to_string()],
        "DELETE FROM target should record target table for RETURNING: focused={:?}",
        ctx.focused_tables
    );
}

#[test]
fn delete_from_with_alias_returning_has_focused_target_table() {
    let ctx = analyze("DELETE FROM employees e WHERE e.id > 0 RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert_eq!(
        ctx.focused_tables,
        vec!["employees".to_string()],
        "focused: {:?}",
        ctx.focused_tables
    );
}

#[test]
fn delete_without_from_returning_has_focused_target_table() {
    // Oracle-style DELETE table_name (without FROM).
    let ctx = analyze("DELETE employees WHERE id > 0 RETURNING |");
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert_eq!(
        ctx.focused_tables,
        vec!["employees".to_string()],
        "focused: {:?}",
        ctx.focused_tables
    );
}

#[test]
fn delete_from_using_returning_focuses_target_table() {
    // PostgreSQL DELETE ... USING: RETURNING should still focus the target table.
    let ctx = analyze(
        "DELETE FROM target_table t USING source_table s \
         WHERE t.id = s.id RETURNING |",
    );
    assert_eq!(ctx.phase, SqlPhase::DmlReturningList);
    assert_eq!(
        ctx.focused_tables,
        vec!["target_table".to_string()],
        "DELETE USING should focus target, not source: focused={:?}",
        ctx.focused_tables
    );
}

#[test]
fn cross_join_table_position_is_table_context() {
    let ctx = analyze("SELECT * FROM t1 CROSS JOIN |");
    assert!(
        ctx.phase.is_table_context(),
        "CROSS JOIN target should be table context, got {:?}",
        ctx.phase
    );
}

#[test]
fn natural_join_table_position_is_table_context() {
    let ctx = analyze("SELECT * FROM t1 NATURAL JOIN |");
    assert!(
        ctx.phase.is_table_context(),
        "NATURAL JOIN target should be table context, got {:?}",
        ctx.phase
    );
}

#[test]
fn left_outer_join_table_position_is_table_context() {
    let ctx = analyze("SELECT * FROM t1 LEFT OUTER JOIN |");
    assert!(
        ctx.phase.is_table_context(),
        "LEFT OUTER JOIN target should be table context, got {:?}",
        ctx.phase
    );
}

#[test]
fn full_join_table_position_is_table_context() {
    let ctx = analyze("SELECT * FROM t1 FULL JOIN |");
    assert!(
        ctx.phase.is_table_context(),
        "FULL JOIN target should be table context, got {:?}",
        ctx.phase
    );
}

// ─── State machine regression tests ─────────────────────────────────────

#[test]
fn pivot_xml_skips_generated_columns() {
    let tokens =
        tokenize("SELECT * FROM sales PIVOT XML (SUM(amount) FOR quarter IN ('Q1' AS Q1))");
    let parsed = parse_top_level_pivot_clause(&tokens).expect("PIVOT XML clause should be parsed");
    assert!(parsed.generated_columns.is_empty());
    assert_eq!(parsed.for_columns, vec!["quarter".to_string()]);
    assert_eq!(parsed.aggregate_columns, vec!["amount".to_string()]);
}

#[test]
fn parse_simple_identifier_path_rejects_trailing_dot() {
    let tokens = [
        SqlToken::Word("schema".to_string()),
        SqlToken::Symbol(".".to_string()),
    ];
    let refs: Vec<&SqlToken> = tokens.iter().collect();
    assert_eq!(parse_simple_identifier_path_output_column(&refs), None);
}

#[test]
fn normalize_dotted_identifier_rejects_double_dot() {
    let tokens = [
        SqlToken::Word("schema".to_string()),
        SqlToken::Symbol(".".to_string()),
        SqlToken::Symbol(".".to_string()),
        SqlToken::Word("table".to_string()),
    ];
    let refs: Vec<&SqlToken> = tokens.iter().collect();
    assert_eq!(normalize_dotted_identifier_tokens(&refs), None);
}

// ─── Oracle grammar coverage regression tests ───────────────────────────

#[test]
fn grammar_deeply_nested_query_variant_1() {
    let ctx = analyze("SELECT * FROM (SELECT * FROM (SELECT * FROM (SELECT | FROM dual)))");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
    assert_eq!(ctx.depth, 3);
}

#[test]
fn grammar_deeply_nested_query_variant_2() {
    let ctx = analyze(
        "SELECT * FROM a WHERE EXISTS (SELECT 1 FROM b WHERE b.id IN (SELECT c.id FROM c WHERE |))",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert_eq!(ctx.depth, 2);
}

#[test]
fn grammar_deeply_nested_query_variant_3() {
    let ctx = analyze(
        "SELECT * FROM (SELECT x, (SELECT COUNT(*) FROM t3 WHERE t3.k = t2.k AND |) AS cnt FROM t2)",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert_eq!(ctx.depth, 2);
}

#[test]
fn grammar_nested_case_variant_1() {
    let ctx = analyze("SELECT CASE WHEN a = 1 THEN CASE WHEN b = 2 THEN | END END FROM t");
    assert_eq!(ctx.phase, SqlPhase::SelectList);
}

#[test]
fn grammar_nested_case_variant_2() {
    let cols = extract_select_list_columns(&tokenize(
        "SELECT CASE WHEN a=1 THEN CASE WHEN b=2 THEN c END END AS nested_case_col FROM t",
    ));
    assert!(
        cols.contains(&"nested_case_col".to_string()),
        "cols: {:?}",
        cols
    );
}

#[test]
fn grammar_nested_case_variant_3() {
    let ctx = analyze("SELECT * FROM t WHERE CASE WHEN a = 1 THEN CASE WHEN b = 2 THEN 1 ELSE 0 END ELSE 0 END = |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn grammar_analytic_window_variant_1() {
    let ctx = analyze("SELECT SUM(sal) OVER (PARTITION BY deptno ORDER BY |) FROM emp");
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
}

#[test]
fn grammar_analytic_window_variant_2() {
    let ctx =
        analyze("SELECT ROW_NUMBER() OVER (PARTITION BY deptno ORDER BY sal) rn FROM emp WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
}

#[test]
fn grammar_analytic_window_variant_3() {
    let cols = extract_select_list_columns(&tokenize(
        "SELECT SUM(sal) OVER (PARTITION BY deptno ORDER BY sal) AS analytic_sum FROM emp",
    ));
    assert!(
        cols.contains(&"analytic_sum".to_string()),
        "cols: {:?}",
        cols
    );
}

#[test]
fn grammar_hierarchical_query_variant_1() {
    let ctx = analyze("SELECT * FROM emp START WITH mgr IS NULL CONNECT BY PRIOR empno = | ");
    assert_eq!(ctx.phase, SqlPhase::ConnectByClause);
}

#[test]
fn grammar_hierarchical_query_variant_2() {
    let ctx = analyze("SELECT * FROM emp CONNECT BY NOCYCLE PRIOR empno = mgr START WITH |");
    assert_eq!(ctx.phase, SqlPhase::StartWithClause);
}

#[test]
fn grammar_hierarchical_query_variant_3() {
    let ctx = analyze(
        "SELECT e.empno FROM emp e WHERE EXISTS (SELECT 1 FROM emp c START WITH c.mgr = e.empno CONNECT BY PRIOR c.empno = c.mgr AND |)",
    );
    assert_eq!(ctx.phase, SqlPhase::ConnectByClause);
    assert_eq!(ctx.depth, 1);
}

#[test]
fn grammar_hierarchical_search_by_clause_stays_column_context() {
    let ctx = analyze(
        "SELECT * FROM emp CONNECT BY PRIOR empno = mgr SEARCH DEPTH FIRST BY | SET ord_seq",
    );
    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());
}

#[test]
fn grammar_hierarchical_search_set_uses_generated_column_name_phase() {
    let ctx =
        analyze("SELECT * FROM emp CONNECT BY PRIOR empno = mgr SEARCH DEPTH FIRST BY empno SET |");
    assert_eq!(ctx.phase, SqlPhase::HierarchicalGeneratedColumnName);
    assert!(!ctx.phase.is_column_context());
    assert!(ctx.phase.is_generated_name_context());
}

#[test]
fn grammar_hierarchical_cycle_set_uses_generated_column_name_phase() {
    let ctx = analyze(
        "SELECT * FROM emp CONNECT BY PRIOR empno = mgr CYCLE empno SET | TO 'Y' DEFAULT 'N'",
    );
    assert_eq!(ctx.phase, SqlPhase::HierarchicalGeneratedColumnName);
    assert!(!ctx.phase.is_column_context());
    assert!(ctx.phase.is_generated_name_context());
}

#[test]
fn grammar_hierarchical_order_siblings_by_keeps_order_by_context() {
    let ctx = analyze("SELECT * FROM emp CONNECT BY PRIOR empno = mgr ORDER SIBLINGS BY |");

    assert_eq!(ctx.phase, SqlPhase::OrderByClause);
    assert!(ctx.phase.is_column_context());

    let names = table_names(&ctx);
    assert!(
        names.iter().any(|name| name == "EMP"),
        "expected EMP table to remain in scope, got {:?}",
        names
    );
}

#[test]
fn grammar_with_recursive_style_variant_1() {
    let ctx = analyze(
        "WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM r WHERE n < 10) SELECT * FROM r WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(cte_names(&ctx).contains(&"R".to_string()));
}

#[test]
fn grammar_with_recursive_style_variant_2() {
    let ctx = analyze("WITH cte(x) AS (SELECT 1 FROM dual), cte2 AS (SELECT x FROM cte) SELECT * FROM cte2 WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(cte_names(&ctx).contains(&"CTE".to_string()));
    assert!(cte_names(&ctx).contains(&"CTE2".to_string()));
}

#[test]
fn grammar_with_recursive_style_variant_3() {
    let ctx =
        analyze("SELECT * FROM (WITH cte AS (SELECT 1 AS x FROM dual) SELECT * FROM cte WHERE |)");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert_eq!(ctx.depth, 1);
}

#[test]
fn grammar_with_table_statement_after_with_keeps_cte_scope() {
    let ctx = analyze("WITH cte AS (SELECT 1 AS id FROM dual) TABLE cte WHERE |");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(cte_names(&ctx).contains(&"CTE".to_string()));
}

#[test]
fn grammar_with_table_statement_after_recursive_with_keeps_cte_scope() {
    let ctx = analyze(
        "WITH RECURSIVE cte(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM cte WHERE n < 3) TABLE cte WHERE |",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    assert!(cte_names(&ctx).contains(&"CTE".to_string()));
}

#[test]
fn grammar_complex_join_variant_1() {
    let ctx = analyze("SELECT * FROM emp e LEFT JOIN dept d ON e.deptno = d.deptno JOIN salgrade s ON e.sal BETWEEN s.losal AND | ");
    assert_eq!(ctx.phase, SqlPhase::JoinCondition);
}

#[test]
fn grammar_complex_join_variant_2() {
    let ctx = analyze(
        "SELECT * FROM emp e CROSS APPLY (SELECT * FROM bonus b WHERE b.empno = e.empno AND |) bx",
    );
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"EMP".to_string()), "tables: {:?}", names);
}

#[test]
fn grammar_complex_join_variant_3() {
    let ctx = analyze("SELECT * FROM emp e JOIN LATERAL (SELECT * FROM dept d WHERE d.deptno = e.deptno AND |) ld ON 1=1");
    assert_eq!(ctx.phase, SqlPhase::WhereClause);
    let names = table_names(&ctx);
    assert!(names.contains(&"EMP".to_string()), "tables: {:?}", names);
}

#[test]
fn grammar_quoted_identifier_variant_1() {
    let cols = extract_select_list_columns(&tokenize(r#"SELECT "Employee Name" FROM emp"#));
    assert!(
        cols.contains(&"Employee Name".to_string()),
        "cols: {:?}",
        cols
    );
}

#[test]
fn grammar_quoted_identifier_variant_2() {
    let cols = extract_select_list_columns(&tokenize(
        r#"SELECT e."Hire Date" AS "Joined Date" FROM emp e"#,
    ));
    assert!(
        cols.contains(&"Joined Date".to_string()),
        "cols: {:?}",
        cols
    );
}

#[test]
fn grammar_quoted_identifier_variant_3() {
    let cols = extract_select_list_columns(&tokenize(r#"SELECT "Dept"."Code" FROM "Dept""#));
    assert!(cols.contains(&"Code".to_string()), "cols: {:?}", cols);
}

#[test]
fn oracle_json_table_alias_collects_function_columns() {
    let ctx = analyze(
        r#"
SELECT jt.|
FROM orders o
CROSS APPLY JSON_TABLE(
  o.payload,
  '$.items[*]'
  COLUMNS (
    item_id NUMBER PATH '$.id',
    item_nm VARCHAR2(100) PATH '$.name'
  )
) jt
"#,
    );

    let tables = resolve_qualifier_tables("jt", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["jt".to_string()]);
    let jt = ctx
        .subqueries
        .iter()
        .find(|subquery| subquery.alias.eq_ignore_ascii_case("jt"))
        .expect("JSON_TABLE alias should be collected as virtual relation");
    let body_tokens = token_range_slice(ctx.statement_tokens.as_ref(), jt.body_range);
    let columns = extract_table_function_columns(body_tokens);
    assert!(
        columns
            .iter()
            .any(|col| col.eq_ignore_ascii_case("item_id")),
        "expected item_id from JSON_TABLE COLUMNS clause, got {:?}",
        columns
    );
    assert!(
        columns
            .iter()
            .any(|col| col.eq_ignore_ascii_case("item_nm")),
        "expected item_nm from JSON_TABLE COLUMNS clause, got {:?}",
        columns
    );
}

#[test]
fn sql_server_openjson_with_clause_alias_collects_virtual_columns() {
    let ctx = analyze(
        r#"
SELECT oj.|
FROM orders o
CROSS APPLY OPENJSON(
  o.payload,
  '$.items'
) WITH (
  item_id int '$.id',
  item_nm nvarchar(100) '$.name'
) oj
"#,
    );

    let tables = resolve_qualifier_tables("oj", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["oj".to_string()]);

    let oj = ctx
        .subqueries
        .iter()
        .find(|subquery| subquery.alias.eq_ignore_ascii_case("oj"))
        .expect("OPENJSON alias should be collected as virtual relation");
    let body_tokens = token_range_slice(ctx.statement_tokens.as_ref(), oj.body_range);
    let columns = extract_table_function_columns(body_tokens);
    assert!(
        columns
            .iter()
            .any(|col| col.eq_ignore_ascii_case("item_id")),
        "expected item_id from OPENJSON WITH clause, got {:?}",
        columns
    );
    assert!(
        columns
            .iter()
            .any(|col| col.eq_ignore_ascii_case("item_nm")),
        "expected item_nm from OPENJSON WITH clause, got {:?}",
        columns
    );
}

#[test]
fn sql_server_openjson_without_with_clause_keeps_function_name_resolution() {
    let ctx = analyze("SELECT oj.| FROM orders o CROSS APPLY OPENJSON(o.payload) oj");
    let tables = resolve_qualifier_tables("oj", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["OPENJSON".to_string()]);
    assert!(
        ctx.subqueries
            .iter()
            .all(|subquery| !subquery.alias.eq_ignore_ascii_case("oj")),
        "OPENJSON without explicit output columns should not become a virtual relation: {:?}",
        ctx.subqueries
            .iter()
            .map(|subquery| &subquery.alias)
            .collect::<Vec<_>>()
    );
}

#[test]
fn oracle_table_function_alias_is_collected() {
    let ctx = analyze("SELECT t.| FROM TABLE(pkg_get_rows(:p_id)) t");
    let tables = resolve_qualifier_tables("t", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["pkg_get_rows".to_string()]);
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|table| table.name.eq_ignore_ascii_case("pkg_get_rows")
                && table.alias.as_deref() == Some("t")),
        "TABLE(...) alias should still be collected: {:?}",
        ctx.tables_in_scope
            .iter()
            .map(|table| (&table.name, &table.alias))
            .collect::<Vec<_>>()
    );
}

#[test]
fn oracle_dblink_table_alias_is_collected() {
    let ctx = analyze("SELECT e.| FROM employees@hr_link e");
    let tables = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["employees@hr_link".to_string()]);
}

#[test]
fn oracle_as_of_clause_preserves_alias_after_modifier() {
    let ctx = analyze("SELECT e.| FROM employees AS OF SCN 12345 e");
    let tables = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["employees".to_string()]);
}

#[test]
fn oracle_partition_clause_preserves_alias_after_modifier() {
    let ctx = analyze("SELECT s.| FROM sales PARTITION (p_202501) s");
    let tables = resolve_qualifier_tables("s", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["sales".to_string()]);
}

#[test]
fn oracle_versions_clause_preserves_alias_after_modifier() {
    let ctx = analyze("SELECT e.| FROM employees VERSIONS BETWEEN SCN MINVALUE AND MAXVALUE e");
    let tables = resolve_qualifier_tables("e", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["employees".to_string()]);
}

#[test]
fn full_dotted_qualifier_resolves_to_exact_relation_before_alias_suffix_match() {
    let ctx = analyze(
        "SELECT schema_a.emp.| FROM schema_a.emp JOIN dept emp ON schema_a.emp.deptno = emp.deptno",
    );
    let tables = resolve_qualifier_tables("schema_a.emp", &ctx.tables_in_scope);
    assert_eq!(tables, vec!["schema_a.emp".to_string()]);
}

#[test]
fn extract_select_list_columns_supports_three_part_identifier_projection() {
    let tokens =
        tokenize("WITH q AS (SELECT hr.employees.employee_id FROM hr.employees) SELECT * FROM q");
    let ctes = parse_ctes(&tokens);
    let q = ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("q"))
        .expect("CTE q should be parsed");
    let body_tokens = token_range_slice(&tokens, q.body_range);
    let cols = extract_select_list_columns(body_tokens);
    assert!(
        cols.contains(&"employee_id".to_string()),
        "three-part identifier projection should map to terminal column name: {:?}",
        cols
    );
}

#[test]
fn extract_select_list_columns_supports_three_part_identifier_in_inline_view() {
    let cols = extract_select_list_columns(&tokenize(
        "SELECT hr.employees.employee_id FROM hr.employees",
    ));
    assert!(
        cols.contains(&"employee_id".to_string()),
        "inline view projection should also keep terminal column name: {:?}",
        cols
    );
}
