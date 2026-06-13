fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn runtime_state_for_test(
    completion_range: Option<(usize, usize)>,
    pending: Option<PendingIntellisense>,
    keyup_generation: u64,
    parse_generation: u64,
) -> Arc<IntellisenseRuntimeState> {
    let runtime = Arc::new(IntellisenseRuntimeState::new());
    runtime.set_completion_range(
        completion_range.map(|(start, end)| IntellisenseCompletionRange::new(start, end)),
    );
    runtime.set_pending_intellisense(pending);
    runtime.set_keyup_generation_for_test(keyup_generation);
    runtime.set_parse_generation_for_test(parse_generation);
    runtime
}

fn load_intellisense_test_file(name: &str) -> &'static str {
    match name {
        "test7.txt" => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/test7.txt")),
        "test8.txt" => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/test8.txt")),
        "test10.txt" => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/test10.txt")),
        "test11.txt" => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/test11.txt")),
        _ => {
            static EXTRA_FILES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
            let cache = EXTRA_FILES.get_or_init(|| Mutex::new(HashMap::new()));
            if let Some(script) = lock_or_recover(cache).get(name).copied() {
                return script;
            }

            let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            path.push("test");
            path.push(name);
            let script = Box::leak(
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .into_boxed_str(),
            );
            lock_or_recover(cache).insert(name.to_string(), script);
            script
        }
    }
}

fn load_mariadb_intellisense_test_file(name: &str) -> &'static str {
    match name {
        "test1.txt" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_mysql/test1.txt"
        )),
        "test2.txt" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_mysql/test2.txt"
        )),
        "test3.txt" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_mysql/test3.txt"
        )),
        "test4.txt" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_mariadb/test4.txt"
        )),
        _ => {
            static EXTRA_FILES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
            let cache = EXTRA_FILES.get_or_init(|| Mutex::new(HashMap::new()));
            if let Some(script) = lock_or_recover(cache).get(name).copied() {
                return script;
            }

            let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            path.push("test_mariadb");
            path.push(name);
            let script = Box::leak(
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .into_boxed_str(),
            );
            lock_or_recover(cache).insert(name.to_string(), script);
            script
        }
    }
}

fn cached_statement_spans_for_test_script(sql: &str) -> Vec<(usize, usize)> {
    static SPANS: OnceLock<Mutex<HashMap<String, Vec<(usize, usize)>>>> = OnceLock::new();
    let cache = SPANS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(spans) = lock_or_recover(cache).get(sql).cloned() {
        return spans;
    }

    let spans = super::query_text::statement_spans_in_text_for_db_type(sql, None);
    lock_or_recover(cache).insert(sql.to_string(), spans.clone());
    spans
}

fn simple_single_statement_bounds(sql: &str) -> Option<(usize, usize)> {
    super::query_text::simple_single_statement_bounds(sql)
}

fn analyze_full_script_marker(
    script_with_cursor: &str,
) -> (String, usize, intellisense_context::CursorContext) {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

    let cursor = script_with_cursor
        .find(CURSOR_MARKER)
        .expect("cursor marker should exist");
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let (stmt_start, stmt_end) = simple_single_statement_bounds(&sql).unwrap_or_else(|| {
        cached_statement_spans_for_test_script(&sql)
            .into_iter()
            .find(|(start, end)| cursor >= *start && cursor < *end)
            .unwrap_or_else(|| SqlEditorWidget::statement_bounds_in_text(&sql, cursor))
    });
    let statement = sql.get(stmt_start..stmt_end).unwrap_or("").to_string();
    let cursor_in_statement = cursor.saturating_sub(stmt_start).min(statement.len());
    let (normalized_statement, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(
            &statement,
            cursor_in_statement,
        );
    let deep_ctx =
        SqlEditorWidget::analyze_statement_context(&normalized_statement, normalized_cursor);
    (normalized_statement, normalized_cursor, deep_ctx)
}

fn analyze_full_script_target_replacement(
    script: &str,
    target: &str,
    replacement: &str,
) -> (String, usize, intellisense_context::CursorContext) {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

    let cursor_in_replacement = replacement
        .find(CURSOR_MARKER)
        .expect("replacement must include cursor marker");
    let target_start = script
        .find(target)
        .unwrap_or_else(|| panic!("expected target to exist in script: {target}"));
    let cursor = target_start.saturating_add(cursor_in_replacement);
    let (stmt_start, stmt_end) = simple_single_statement_bounds(script).unwrap_or_else(|| {
        cached_statement_spans_for_test_script(script)
            .into_iter()
            .find(|(start, end)| cursor >= *start && cursor < *end)
            .unwrap_or_else(|| SqlEditorWidget::statement_bounds_in_text(script, cursor))
    });
    let statement = script.get(stmt_start..stmt_end).unwrap_or("").to_string();
    let cursor_in_statement = cursor.saturating_sub(stmt_start).min(statement.len());
    let (normalized_statement, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(
            &statement,
            cursor_in_statement,
        );
    let deep_ctx =
        SqlEditorWidget::analyze_statement_context(&normalized_statement, normalized_cursor);
    (normalized_statement, normalized_cursor, deep_ctx)
}

fn analyze_inline_cursor_sql(sql_with_cursor: &str) -> intellisense_context::CursorContext {
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    intellisense_context::analyze_cursor_context_owned(full_tokens, split_idx)
}

fn mysql_context_and_suggestions_for_inline_sql(
    sql_with_cursor: &str,
) -> (SqlContext, Vec<String>) {
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context_owned(full_tokens, split_idx);
    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let mut data = IntellisenseData::new();
    let suggestions = SqlEditorWidget::base_suggestions_for_context(
        &mut data,
        &prefix,
        None,
        None,
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        context,
        false,
        Some(crate::db::DatabaseType::MySQL),
    );

    (context, suggestions)
}

fn assert_has_case_insensitive(values: &[String], expected: &str) {
    assert!(
        values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(expected)),
        "expected `{expected}` in values: {:?}",
        values
    );
}

fn virtual_columns_for<'a>(
    columns_by_name: &'a HashMap<String, Vec<String>>,
    relation_name: &str,
) -> &'a Vec<String> {
    columns_by_name
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(relation_name))
        .map(|(_, columns)| columns)
        .unwrap_or_else(|| {
            panic!(
                "expected virtual columns for `{relation_name}`, got keys: {:?}",
                columns_by_name.keys().collect::<Vec<_>>()
            )
        })
}

fn collect_virtual_columns_from_ctes(
    deep_ctx: &intellisense_context::CursorContext,
    data: &Arc<Mutex<IntellisenseData>>,
    sender: &mpsc::Sender<ColumnLoadUpdate>,
    connection: &SharedConnection,
) -> HashMap<String, Vec<String>> {
    let mut virtual_table_columns = HashMap::new();
    for cte in &deep_ctx.ctes {
        let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
            deep_ctx,
            cte,
            &virtual_table_columns,
            data,
            sender,
            connection,
        );
        if !columns.is_empty() {
            SqlEditorWidget::insert_virtual_table_columns(
                &mut virtual_table_columns,
                &cte.name,
                columns,
            );
        }
    }
    virtual_table_columns
}

fn collect_virtual_columns_from_relations(
    deep_ctx: &intellisense_context::CursorContext,
    data: &Arc<Mutex<IntellisenseData>>,
    sender: &mpsc::Sender<ColumnLoadUpdate>,
    connection: &SharedConnection,
) -> HashMap<String, Vec<String>> {
    let mut virtual_table_columns =
        collect_virtual_columns_from_ctes(deep_ctx, data, sender, connection);

    for subq in &deep_ctx.subqueries {
        if let Some(columns) =
            SqlEditorWidget::explicit_subquery_columns_for_completion(deep_ctx, subq)
        {
            SqlEditorWidget::insert_virtual_table_columns(
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
        let body_ctx = intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
        let mut body_virtual_table_columns = virtual_table_columns.clone();
        for cte in &body_ctx.ctes {
            let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
                &body_ctx,
                cte,
                &body_virtual_table_columns,
                data,
                sender,
                connection,
            );
            if !columns.is_empty() {
                SqlEditorWidget::insert_virtual_table_columns(
                    &mut body_virtual_table_columns,
                    &cte.name,
                    columns,
                );
            }
        }
        let (columns, _) = SqlEditorWidget::collect_virtual_relation_columns_for_completion(
            body_tokens,
            &body_ctx.tables_in_scope,
            &deep_ctx.tables_in_scope,
            &body_virtual_table_columns,
            data,
            sender,
            connection,
        );
        if !columns.is_empty() {
            SqlEditorWidget::insert_virtual_table_columns(
                &mut virtual_table_columns,
                &subq.alias,
                columns,
            );
        }
    }

    virtual_table_columns
}

#[test]
fn virtual_table_columns_lookup_matches_quoted_alias() {
    let mut virtual_table_columns = HashMap::new();
    SqlEditorWidget::insert_virtual_table_columns(
        &mut virtual_table_columns,
        r#""Sales Alias""#,
        vec!["order_id".to_string()],
    );

    assert_eq!(
        SqlEditorWidget::virtual_table_columns_for_lookup(
            &virtual_table_columns,
            r#""Sales Alias""#
        ),
        Some(["order_id".to_string()].as_slice())
    );
    assert_eq!(
        SqlEditorWidget::virtual_table_columns_for_lookup(&virtual_table_columns, "Sales Alias"),
        Some(["order_id".to_string()].as_slice())
    );
}

#[test]
fn column_load_worker_pool_enqueue_returns_err_when_worker_pool_is_empty() {
    let pool = ColumnLoadWorkerPool {
        worker_senders: Vec::new(),
        worker_handles: Mutex::new(Vec::new()),
        next_worker: AtomicUsize::new(0),
    };
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let task = ColumnLoadTask {
        table_key: "EMP".to_string(),
        connection: create_shared_connection(),
        sender,
        foreign_keys: false,
    };

    let result = pool.enqueue(task.clone());
    assert!(result.is_err());
    assert_eq!(
        result.err().map(|value| value.table_key),
        Some(task.table_key)
    );
}

#[test]
fn test7_set_operator_order_by_keeps_compound_statement_context() {
    let script = load_intellisense_test_file("test7.txt");

    for target in [
        "SELECT empno FROM b\nORDER BY __CODEX_CURSOR__empno;",
        "SELECT empno FROM b\nORDER BY __CODEX_CURSOR__empno;\n\nPROMPT [DONE]",
    ] {
        let marked = script.replacen(target.replace("__CODEX_CURSOR__", "").as_str(), target, 1);
        assert_ne!(marked, script, "expected target to exist in test7.txt");
        let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

        assert!(
            statement.contains("INTERSECT") || statement.contains("MINUS"),
            "compound set-operator statement should be preserved, got:\n{statement}"
        );
        assert!(
            statement.contains("ORDER BY empno"),
            "ORDER BY should remain inside the same statement, got:\n{statement}"
        );
        assert_eq!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause,
            "cursor inside set-operator ORDER BY should stay in OrderByClause"
        );
    }
}

#[test]
fn test7_match_recognize_generated_columns_are_extracted_from_full_script_statement() {
    let script = load_intellisense_test_file("test7.txt");
    let marked = script.replacen(
        "FIRST(ename) AS start_name,",
        "FIRST(ename) AS __CODEX_CURSOR__start_name,",
        1,
    );
    assert_ne!(
        marked, script,
        "expected MATCH_RECOGNIZE target in test7.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("MATCH_RECOGNIZE"),
        "current statement should contain MATCH_RECOGNIZE, got:\n{statement}"
    );

    let generated = intellisense_context::extract_match_recognize_generated_columns(
        deep_ctx.statement_tokens.as_ref(),
    );
    for expected in ["start_name", "end_name", "run_len"] {
        assert_has_case_insensitive(&generated, expected);
    }
}

#[test]
fn test7_nested_inline_view_wildcard_expands_columns_from_nested_cte() {
    let script = load_intellisense_test_file("test7.txt");
    let marked = script.replacen(
        "ORDER BY v.amt DESC, v.order_dt;",
        "ORDER BY v.__CODEX_CURSOR__amt DESC, v.order_dt;",
        1,
    );
    assert_ne!(marked, script, "expected inline-view target in test7.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    let v_subquery = deep_ctx
        .subqueries
        .iter()
        .find(|subq| subq.alias.eq_ignore_ascii_case("v"))
        .expect("expected inline view alias v");
    let body_tokens = intellisense_context::token_range_slice(
        deep_ctx.statement_tokens.as_ref(),
        v_subquery.body_range,
    );
    let body_ctx = intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
    let mut body_virtual_table_columns = virtual_table_columns.clone();
    for cte in &body_ctx.ctes {
        let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
            &body_ctx,
            cte,
            &body_virtual_table_columns,
            &data,
            &sender,
            &connection,
        );
        if !columns.is_empty() {
            SqlEditorWidget::insert_virtual_table_columns(
                &mut body_virtual_table_columns,
                &cte.name,
                columns,
            );
        }
    }
    let body_tables_in_scope = body_ctx.tables_in_scope.clone();
    let (wildcard_columns, wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
        body_tokens,
        &body_tables_in_scope,
        &body_virtual_table_columns,
        &data,
        &sender,
        &connection,
    );

    assert_eq!(wildcard_tables, vec!["x".to_string()]);
    for expected in ["order_id", "cust_name", "order_dt", "amt"] {
        assert_has_case_insensitive(&wildcard_columns, expected);
    }
}

#[test]
fn test8_package_body_select_context_stays_inside_open_rc_query() {
    let script = load_intellisense_test_file("test8.txt");
    let marked = script.replacen(
        "                t.grp,",
        "                t.__CODEX_CURSOR__grp,",
        1,
    );
    assert_ne!(
        marked, script,
        "expected open_rc SELECT target in test8.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("PROCEDURE open_rc"),
        "cursor should stay inside package body statement, got:\n{statement}"
    );
    assert!(
        statement.contains("FROM oqt_t_test t"),
        "open_rc query should remain in scope, got:\n{statement}"
    );
    let column_tables =
        intellisense_context::resolve_qualifier_tables("t", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["oqt_t_test".to_string()]);
}

#[test]
fn test8_summary_query_statement_isolated_after_plsql_and_print() {
    let script = load_intellisense_test_file("test8.txt");
    let marked = script.replacen(
        "    COUNT (*) AS cnt,",
        "    COUNT (*) AS __CODEX_CURSOR__cnt,",
        1,
    );
    assert_ne!(marked, script, "expected summary query target in test8.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.starts_with("SELECT grp,"),
        "summary query should start at final SELECT, got:\n{statement}"
    );
    assert!(
        statement.contains("FROM oqt_t_test"),
        "summary query should include oqt_t_test, got:\n{statement}"
    );
    assert!(
        !statement.contains("PRINT v_rc"),
        "summary query statement should not include preceding PRINT command:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);
    let tables: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|table| table.name.to_ascii_uppercase())
        .collect();
    assert!(tables.contains(&"OQT_T_TEST".to_string()));
}

#[test]
fn test8_log_query_order_by_statement_isolated_from_previous_summary_query() {
    let script = load_intellisense_test_file("test8.txt");
    let order_by_prefix = "ORDER BY ";
    let order_by_target = "ORDER BY LOG_ID";
    let marked = script
        .to_ascii_uppercase()
        .find(order_by_target)
        .map(|target_start| {
            let insert_at = target_start.saturating_add(order_by_prefix.len());
            let mut marked =
                String::with_capacity(script.len().saturating_add("__CODEX_CURSOR__".len()));
            marked.push_str(&script[..insert_at]);
            marked.push_str("__CODEX_CURSOR__");
            marked.push_str(&script[insert_at..]);
            marked
        })
        .unwrap_or_else(|| script.to_string());
    assert_ne!(
        marked, script,
        "expected log query ORDER BY target in test8.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);
    let statement_upper = statement.to_ascii_uppercase();

    assert!(
        statement_upper.contains("FROM OQT_T_LOG"),
        "log query should include oqt_t_log, got:\n{statement}"
    );
    assert!(
        statement_upper.contains("FETCH FIRST 40 ROWS ONLY"),
        "log query should preserve trailing FETCH clause, got:\n{statement}"
    );
    assert!(
        !statement_upper.contains("FROM OQT_T_TEST"),
        "log query should not leak previous summary query:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );
}

#[test]
fn test10_with_function_statement_isolated_after_bulk_collect_block() {
    let script = load_intellisense_test_file("test10.txt");
    let marked = script.replacen(
        "    calc_bonus (NVL (e.salary, 0)) AS calc_bonus",
        "    calc_bonus (NVL (e.salary, 0)) AS __CODEX_CURSOR__calc_bonus",
        1,
    );
    assert_ne!(
        marked, script,
        "expected WITH FUNCTION target in test10.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("WITH FUNCTION calc_bonus"),
        "WITH FUNCTION statement should remain isolated, got:\n{statement}"
    );
    assert!(
        statement.contains("FROM qt_emp e"),
        "WITH FUNCTION query should include qt_emp alias e, got:\n{statement}"
    );
    assert!(
        !statement.contains("FETCH c_emp BULK COLLECT"),
        "WITH FUNCTION statement should not include previous PL/SQL block:\n{statement}"
    );
    let column_tables =
        intellisense_context::resolve_qualifier_tables("e", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["qt_emp".to_string()]);
}

#[test]
fn test10_recursive_with_statement_keeps_ctes_and_order_by() {
    let script = load_intellisense_test_file("test10.txt");
    let marked = script.replacen("    r.dept_rank,", "    r.__CODEX_CURSOR__dept_rank,", 1);
    assert_ne!(
        marked, script,
        "expected recursive WITH target in test10.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("WITH dept_tree"),
        "recursive WITH statement should include dept_tree CTE, got:\n{statement}"
    );
    assert!(
        statement.contains("sales_ranked AS"),
        "recursive WITH statement should include sales_ranked CTE, got:\n{statement}"
    );
    assert!(
        statement.contains("ORDER BY t.path_txt"),
        "recursive WITH statement should preserve final ORDER BY, got:\n{statement}"
    );
    let tables: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|table| table.name.to_ascii_uppercase())
        .collect();
    assert!(tables.contains(&"DEPT_TREE".to_string()));
    assert!(tables.contains(&"SALES_RANKED".to_string()));
}

#[test]
fn test10_cross_apply_alias_columns_resolve_in_full_script() {
    let script = load_intellisense_test_file("test10.txt");
    let marked = script.replacen("    x.max_amt,", "    x.__CODEX_CURSOR__max_amt,", 1);
    assert_ne!(marked, script, "expected CROSS APPLY target in test10.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    let column_tables =
        intellisense_context::resolve_qualifier_tables("x", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["x".to_string()]);

    let x_subquery = deep_ctx
        .subqueries
        .iter()
        .find(|subq| subq.alias.eq_ignore_ascii_case("x"))
        .expect("expected CROSS APPLY alias x");
    let body_tokens = intellisense_context::token_range_slice(
        deep_ctx.statement_tokens.as_ref(),
        x_subquery.body_range,
    );
    let columns = intellisense_context::extract_select_list_columns(body_tokens);
    for expected in ["max_amt", "min_amt"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn test10_pipelined_table_query_isolated_from_adjacent_final_queries() {
    let script = load_intellisense_test_file("test10.txt");
    let marked = script.replacen(
        "FROM TABLE (qt_pipe_emp (NULL))\nORDER BY emp_id;",
        "FROM TABLE (qt_pipe_emp (NULL))\nORDER BY __CODEX_CURSOR__emp_id;",
        1,
    );
    assert_ne!(
        marked, script,
        "expected final TABLE(...) query target in test10.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("FROM TABLE (qt_pipe_emp (NULL))"),
        "TABLE(...) statement should be isolated, got:\n{statement}"
    );
    assert!(
        !statement.contains("json_like_report"),
        "TABLE(...) statement should not include previous final validation query:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );
}

#[test]
fn test11_with_function_statement_isolated_after_package_execution_block() {
    let script = load_intellisense_test_file("test11.txt");
    let marked = script.replacen(
        "    score_fn (e.salary, e.bonus_pct) AS score",
        "    score_fn (e.salary, e.bonus_pct) AS __CODEX_CURSOR__score",
        1,
    );
    assert_ne!(
        marked, script,
        "expected WITH FUNCTION target in test11.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.trim_start().starts_with("WITH") && statement.contains("FUNCTION score_fn"),
        "WITH FUNCTION statement should remain isolated, got:\n{statement}"
    );
    assert!(
        statement.contains("FROM qt_employees e"),
        "WITH FUNCTION query should include qt_employees alias e, got:\n{statement}"
    );
    assert!(
        !statement.contains("qt_torture_pkg.complex_block"),
        "WITH FUNCTION statement should not include previous PL/SQL block:\n{statement}"
    );
    let column_tables =
        intellisense_context::resolve_qualifier_tables("e", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["qt_employees".to_string()]);
}

#[test]
fn test11_recursive_with_search_cycle_statement_keeps_cte_and_order_by() {
    let script = load_intellisense_test_file("test11.txt");
    let marked = script.replacen("    dfs_ord,", "    __CODEX_CURSOR__dfs_ord,", 1);
    assert_ne!(
        marked, script,
        "expected recursive WITH target in test11.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("WITH dept_tree"),
        "recursive WITH statement should include dept_tree CTE, got:\n{statement}"
    );
    assert!(
        statement.contains("SEARCH DEPTH FIRST BY dept_id"),
        "recursive WITH statement should preserve SEARCH clause, got:\n{statement}"
    );
    assert!(
        statement.contains("ORDER BY dfs_ord"),
        "recursive WITH statement should preserve final ORDER BY, got:\n{statement}"
    );
    let tables: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|table| table.name.to_ascii_uppercase())
        .collect();
    assert!(tables.contains(&"DEPT_TREE".to_string()));
}

#[test]
fn test11_match_recognize_generated_columns_are_extracted_from_full_script_statement() {
    let script = load_intellisense_test_file("test11.txt");
    let marked = script.replacen(
        "MATCH_NUMBER () AS match_no,",
        "MATCH_NUMBER () AS __CODEX_CURSOR__match_no,",
        1,
    );
    assert_ne!(
        marked, script,
        "expected MATCH_RECOGNIZE target in test11.txt"
    );
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("MATCH_RECOGNIZE"),
        "MATCH_RECOGNIZE statement should remain isolated, got:\n{statement}"
    );
    let generated = intellisense_context::extract_match_recognize_generated_columns(
        deep_ctx.statement_tokens.as_ref(),
    );
    for expected in ["match_no", "cls", "start_dt", "end_dt", "total_amt"] {
        assert_has_case_insensitive(&generated, expected);
    }
}

#[test]
fn test11_json_table_statement_exposes_table_function_columns() {
    let script = load_intellisense_test_file("test11.txt");
    let marked = script.replacen(
        "ORDER BY jt.emp_id,",
        "ORDER BY jt.__CODEX_CURSOR__emp_id,",
        1,
    );
    assert_ne!(marked, script, "expected JSON_TABLE target in test11.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("FROM JSON_TABLE"),
        "JSON_TABLE statement should remain isolated, got:\n{statement}"
    );
    let column_tables =
        intellisense_context::resolve_qualifier_tables("jt", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["jt".to_string()]);
    let jt_subquery = deep_ctx
        .subqueries
        .iter()
        .find(|subq| subq.alias.eq_ignore_ascii_case("jt"))
        .expect("expected JSON_TABLE alias jt");
    let body_tokens = intellisense_context::token_range_slice(
        deep_ctx.statement_tokens.as_ref(),
        jt_subquery.body_range,
    );
    let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
    if columns.is_empty() {
        columns = intellisense_context::extract_table_function_columns(body_tokens);
    }
    for expected in ["emp_id", "skill"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn test11_table_function_query_isolated_from_adjacent_queries() {
    let script = load_intellisense_test_file("test11.txt");
    let marked = script.replacen(
        "FROM TABLE (qt_torture_pkg.pipe_sales (NULL))\nORDER BY sale_id;",
        "FROM TABLE (qt_torture_pkg.pipe_sales (NULL))\nORDER BY __CODEX_CURSOR__sale_id;",
        1,
    );
    assert_ne!(marked, script, "expected TABLE(...) target in test11.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(&marked);

    assert!(
        statement.contains("FROM TABLE (qt_torture_pkg.pipe_sales (NULL))"),
        "TABLE(...) statement should be isolated, got:\n{statement}"
    );
    assert!(
        !statement.contains("XMLTABLE"),
        "TABLE(...) statement should not include previous XMLTABLE query:\n{statement}"
    );
    assert!(
        !statement.contains("qt_complex_v"),
        "TABLE(...) statement should not include following view query:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );
}

#[test]
fn statement_bounds_ignore_semicolon_in_string_literal() {
    let sql = "SELECT 'a;b' AS txt FROM dual; SELECT 2 FROM dual";
    let cursor = sql.find("FROM dual").unwrap_or(0);
    let (start, end) = SqlEditorWidget::statement_bounds_in_text(sql, cursor);
    assert_eq!(
        sql.get(start..end).unwrap_or(""),
        "SELECT 'a;b' AS txt FROM dual"
    );
}

#[test]
fn raw_cursor_byte_offset_clamps_negative_offsets_to_first_statement() {
    let sql = "SELECT 1 FROM dual;\nSELECT 2 FROM dual;";
    let cursor_byte = SqlEditorWidget::raw_cursor_byte_offset(-12, sql.len() as i32);
    assert_eq!(cursor_byte, 0);
    assert_eq!(
        super::query_text::statement_at_cursor(sql, cursor_byte).as_deref(),
        Some("SELECT 1 FROM dual")
    );
}

#[test]
fn statement_bounds_ignore_inner_plsql_semicolons() {
    let sql = "BEGIN\n  v := 1;\n  v := v + 1;\nEND;\nSELECT * FROM dual;";
    let cursor = sql.find("v + 1").unwrap_or(0);
    let (start, end) = SqlEditorWidget::statement_bounds_in_text(sql, cursor);
    assert_eq!(
        sql.get(start..end).unwrap_or(""),
        "BEGIN\n  v := 1;\n  v := v + 1;\nEND"
    );
}

#[test]
fn statement_context_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
    let sql = "SELECT 5--2;\nSELECT 9;\n";
    let cursor = sql.find("5--2").unwrap_or(0);
    let context = SqlEditorWidget::statement_context_in_text_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    );

    assert_eq!(
        context,
        "SELECT 5--2",
        "intellisense statement context must keep MySQL `--<non-space>` arithmetic inside the active statement"
    );
}

#[test]
fn expanded_statement_window_for_mysql_db_type_keeps_double_dash_arithmetic_as_code() {
    let sql = "SELECT 5--2;\nSELECT 9;\n";
    let cursor = sql.find("5--2").unwrap_or(0);
    let expanded = SqlEditorWidget::expanded_statement_window_in_text_for_db_type(
        sql,
        cursor,
        Some(crate::db::connection::DatabaseType::MySQL),
    );

    assert_eq!(
        expanded.text,
        "SELECT 5--2",
        "local symbol statement window must keep MySQL `--<non-space>` arithmetic inside the active statement"
    );
}

#[test]
fn mariadb_final_boss_ranked_cte_completion_context_survives_full_script_split() {
    let script = load_mariadb_intellisense_test_file("test1.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ORDER BY order_id",
        "ORDER BY __CODEX_CURSOR__order_id",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_final_boss ()"),
        "cursor should stay inside the final-boss procedure statement, got:\n{statement}"
    );
    assert!(
        statement.contains("WITH order_base AS (") && statement.contains("FROM ranked"),
        "ranked CTE query should remain in scope, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_columns = collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);
    let ranked_columns = virtual_columns_for(&virtual_columns, "ranked");

    for expected in [
        "order_id",
        "emp_id",
        "total_usd",
        "created_at",
        "global_rank",
    ] {
        assert_has_case_insensitive(ranked_columns, expected);
    }
}

#[test]
fn mariadb_parser_killer_ranked_cte_completion_context_survives_full_script_split() {
    let script = load_mariadb_intellisense_test_file("test2.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "SELECT\n        owner_name,\n        weight_sum\n    INTO",
        "SELECT\n        __CODEX_CURSOR__owner_name,\n        weight_sum\n    INTO",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_parser_killer ()"),
        "cursor should stay inside the parser-killer procedure statement, got:\n{statement}"
    );
    assert!(
        statement.contains("WITH owner_score AS (") && statement.contains("FROM ranked"),
        "ranked CTE query should remain in scope, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_columns = collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);
    let ranked_columns = virtual_columns_for(&virtual_columns, "ranked");

    for expected in ["owner_name", "task_cnt", "priority_sum", "weight_sum", "rn"] {
        assert_has_case_insensitive(ranked_columns, expected);
    }
}

#[test]
fn mariadb_ultra_final_boss_ranked_cte_completion_context_survives_full_script_split() {
    let script = load_mariadb_intellisense_test_file("test3.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "WHERE owner_name = 'alice';",
        "WHERE __CODEX_CURSOR__owner_name = 'alice';",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_ultra_final_boss ()"),
        "cursor should stay inside the ultra-final procedure statement, got:\n{statement}"
    );
    assert!(
        statement.contains("WITH run_minutes AS (")
            && statement.contains("WINDOW")
            && statement.contains("FROM ranked"),
        "window-ranked CTE query should remain in scope, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::WhereClause);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_columns = collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);
    let ranked_columns = virtual_columns_for(&virtual_columns, "ranked");

    for expected in [
        "run_id",
        "owner_name",
        "weighted_minutes",
        "rn_in_owner",
        "prev_weighted_minutes",
        "running_owner_weighted",
        "global_rank",
    ] {
        assert_has_case_insensitive(ranked_columns, expected);
    }
}

// ─── Additional MariaDB/MySQL intellisense tests ─────────────────────────────

#[test]
fn mariadb_final_boss_window_named_window_definition_is_column_context() {
    // test1.txt: cursor inside WINDOW named-window definition body.
    // After `WINDOW w_emp AS (PARTITION BY ob.|emp_id ...)`, the phase must
    // be OrderByClause and table alias `ob` must be visible.
    let script = load_mariadb_intellisense_test_file("test1.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "PARTITION BY ob.emp_id\n                ORDER BY ob.created_at, ob.order_id\n            ),",
        "PARTITION BY ob.__CODEX_CURSOR__emp_id\n                ORDER BY ob.created_at, ob.order_id\n            ),",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_final_boss ()"),
        "cursor should stay inside the final-boss procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause,
        "WINDOW definition body should be OrderByClause phase"
    );

    let table_names: Vec<String> = deep_ctx
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
        table_names.iter().any(|n| n == "OB"),
        "alias `ob` (for order_base) must be visible inside WINDOW definition, got: {table_names:?}"
    );
}

#[test]
fn mariadb_final_boss_recursive_cte_union_all_member_select_is_select_list() {
    // test1.txt: cursor inside the recursive UNION ALL member SELECT of dept_tree.
    // `SELECT c.dept_id, c.parent_dept_id, c.dept_code, CONCAT(p.path_txt, ' > ', c.dept_code) ...`
    let script = load_mariadb_intellisense_test_file("test1.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "CONCAT(p.path_txt, ' > ', c.dept_code) AS path_txt,",
        "CONCAT(p.path_txt, ' > ', c.__CODEX_CURSOR__dept_code) AS path_txt,",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectList,
        "UNION ALL member SELECT should be in SelectList phase"
    );
    // dept and dept_tree (self-ref) must be visible in the UNION ALL member scope.
    let table_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        table_names.iter().any(|n| n == "DEPT"),
        "table `dept` (as c) must be visible in recursive CTE member, got: {table_names:?}"
    );
}

#[test]
fn mariadb_parser_killer_exists_subquery_where_is_where_clause() {
    // test2.txt: cursor inside WHERE clause of an EXISTS subquery.
    // `SELECT 1 FROM task AS t WHERE t.node_id = n.|node_id`
    let script = load_mariadb_intellisense_test_file("test2.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "WHERE t.node_id = n.node_id",
        "WHERE t.node_id = n.__CODEX_CURSOR__node_id",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::WhereClause,
        "EXISTS subquery WHERE clause should be WhereClause phase"
    );
    // table `n` (alias of node) should be visible as outer reference
    let qualifier_tables =
        intellisense_context::resolve_qualifier_tables("n", &deep_ctx.tables_in_scope);
    assert!(
        !qualifier_tables.is_empty(),
        "qualifier `n` must resolve inside EXISTS subquery, got empty"
    );
}

#[test]
fn mariadb_parser_killer_while_loop_body_select_is_where_clause() {
    // test2.txt: the sp_run_parser_killer procedure contains a WITH ... SELECT
    // statement after several control-flow blocks.  Cursor at the WHERE clause
    // of the scalar SELECT inside the procedure body.
    let script = load_mariadb_intellisense_test_file("test2.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "FROM agg_result\n     WHERE result_key = 'TEMP_ROLLBACK'",
        "FROM agg_result\n     WHERE result_key = '__CODEX_CURSOR__TEMP_ROLLBACK'",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_parser_killer ()"),
        "cursor should stay inside the parser-killer procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::WhereClause,
        "scalar SELECT WHERE clause should be WhereClause phase"
    );
    let table_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        table_names.iter().any(|n| n == "AGG_RESULT"),
        "table `agg_result` must be in scope, got: {table_names:?}"
    );
}

#[test]
fn mariadb_ultra_final_boss_window_named_window_definition_is_column_context() {
    // test3.txt: cursor inside WINDOW w_owner definition in the ranked CTE body.
    // `WINDOW w_owner AS (PARTITION BY s.|owner_name ORDER BY ...)`
    let script = load_mariadb_intellisense_test_file("test3.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "PARTITION BY s.owner_name\n                ORDER BY s.created_at, s.run_id\n            ),\n            w_owner_running AS (",
        "PARTITION BY s.__CODEX_CURSOR__owner_name\n                ORDER BY s.created_at, s.run_id\n            ),\n            w_owner_running AS (",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_ultra_final_boss ()"),
        "cursor should stay inside the ultra-final procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause,
        "WINDOW definition body should be OrderByClause phase"
    );
    // alias `s` (scored CTE) must be visible inside the window definition
    let qualifier_tables =
        intellisense_context::resolve_qualifier_tables("s", &deep_ctx.tables_in_scope);
    assert!(
        !qualifier_tables.is_empty(),
        "qualifier `s` (scored CTE alias) must resolve inside WINDOW definition, got empty"
    );
}

#[test]
fn mariadb_ultra_final_boss_recursive_cte_second_member_where_clause() {
    // test3.txt: cursor in WHERE of the recursive CTE join condition
    // `JOIN node_tree AS p ON c.parent_node_id = p.|node_id`
    let script = load_mariadb_intellisense_test_file("test3.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ON c.parent_node_id = p.node_id",
        "ON c.parent_node_id = p.__CODEX_CURSOR__node_id",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinCondition,
        "recursive CTE JOIN ON clause should be JoinCondition phase"
    );
    // Both `c` (stage_node) and `p` (node_tree self-ref) must be visible
    let qualifier_p =
        intellisense_context::resolve_qualifier_tables("p", &deep_ctx.tables_in_scope);
    let qualifier_c =
        intellisense_context::resolve_qualifier_tables("c", &deep_ctx.tables_in_scope);
    assert!(
        !qualifier_p.is_empty(),
        "qualifier `p` (node_tree self-ref) must be visible in recursive CTE JOIN ON, got empty"
    );
    assert!(
        !qualifier_c.is_empty(),
        "qualifier `c` (stage_node) must be visible in recursive CTE JOIN ON, got empty"
    );
}

#[test]
fn mariadb_ultra_final_boss_insert_column_list_with_backtick_column() {
    // test3.txt: cursor inside INSERT INTO qa_summary (..., `group`, ...) column list.
    // The backtick-quoted column should not break InsertColumnList phase detection.
    let script = load_mariadb_intellisense_test_file("test3.txt");
    // Target the last INSERT INTO qa_summary column list (uses `group`, `rank`)
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "INSERT INTO qa_summary (\n        summary_key,\n        `group`,\n        `rank`,\n        summary_num,\n        summary_text,\n        summary_json\n    )\n    VALUES\n        (\n            'TOP_OWNER_WEIGHTED'",
        "INSERT INTO qa_summary (\n        summary_key,\n        `group`,\n        `rank`,\n        summary_num,\n        summary_text,\n        __CODEX_CURSOR__summary_json\n    )\n    VALUES\n        (\n            'TOP_OWNER_WEIGHTED'",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_ultra_final_boss ()"),
        "cursor should stay inside the ultra-final procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList,
        "INSERT column list with backtick-quoted columns should be InsertColumnList phase"
    );
}

#[test]
fn mariadb_ultra_final_boss_on_duplicate_key_update_backtick_column_is_dml_set() {
    // test3.txt: cursor inside ON DUPLICATE KEY UPDATE after backtick column.
    // `ON DUPLICATE KEY UPDATE `group` = VALUES(`group`), `rank` = VALUES(`rank`), ...|`
    let script = load_mariadb_intellisense_test_file("test3.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ON DUPLICATE KEY UPDATE\n        `group` = VALUES(`group`),\n        `rank` = VALUES(`rank`),\n        summary_num = VALUES(summary_num),",
        "ON DUPLICATE KEY UPDATE\n        `group` = VALUES(`group`),\n        `rank` = VALUES(`rank`),\n        __CODEX_CURSOR__summary_num = VALUES(summary_num),",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_ultra_final_boss ()"),
        "cursor should stay inside the ultra-final procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DmlSetTargetList,
        "ON DUPLICATE KEY UPDATE with backtick columns should remain DmlSetTargetList"
    );
    assert_eq!(
        deep_ctx.focused_tables,
        vec!["qa_summary".to_string()],
        "focused table for ON DUPLICATE KEY UPDATE should be qa_summary"
    );
}

#[test]
fn mariadb_ultra_final_boss_nested_labeled_block_select_into_is_select_list() {
    // test3.txt: cursor inside a SELECT INTO statement that follows a nested
    // labeled block (`nested_block: BEGIN ... END`).
    // The nested block is terminated by `END;` and the subsequent SELECT after
    // several CALL/WHILE statements should still be found correctly.
    let script = load_mariadb_intellisense_test_file("test3.txt");
    // SELECT MAX(running_owner_weighted) INTO v_alice_running_weighted FROM ranked WHERE owner_name = 'alice'
    // The first occurrence of "MAX(running_owner_weighted)" is in the procedure
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "SELECT MAX(running_owner_weighted)",
        "SELECT __CODEX_CURSOR__MAX(running_owner_weighted)",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_run_ultra_final_boss ()"),
        "cursor should stay inside the ultra-final procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectList,
        "SELECT inside procedure after nested block should be SelectList"
    );

    let cte_names: Vec<String> = deep_ctx
        .ctes
        .iter()
        .map(|c| c.name.to_uppercase())
        .collect();
    assert!(
        cte_names.iter().any(|n| n == "RANKED"),
        "CTE `ranked` must be visible after nested block, got: {cte_names:?}"
    );
}

#[test]
fn mariadb_final_boss_create_or_replace_view_select_list_is_column_context() {
    // test4.txt: cursor inside the SELECT list of the CREATE OR REPLACE VIEW.
    // `SELECT e.employee_id, e.emp_code, CONCAT(e.last_name, ...`
    // REPLACE in `CREATE OR REPLACE VIEW` must NOT be treated as a DML REPLACE.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "e.employee_id,",
        "e.__CODEX_CURSOR__employee_id,",
    );

    assert!(
        statement.starts_with("CREATE OR REPLACE VIEW"),
        "cursor should stay inside the CREATE OR REPLACE VIEW statement, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectList,
        "CREATE OR REPLACE VIEW body should be SelectList phase"
    );
    let table_names: Vec<String> = deep_ctx
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
        "alias `e` (for employees) must be in scope inside CREATE OR REPLACE VIEW, got: {table_names:?}"
    );
    // VIEW must not appear as a relation — it is a DDL keyword, not a table name.
    let raw_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        !raw_names.iter().any(|n| n == "VIEW"),
        "`VIEW` keyword must not be registered as a relation in CREATE OR REPLACE VIEW: {raw_names:?}"
    );
}

#[test]
fn mariadb_final_boss_create_or_replace_view_join_on_is_join_condition() {
    // test4.txt: cursor inside an ON condition of a JOIN inside the CREATE OR REPLACE VIEW body.
    // `JOIN departments d ON d.dept_id = e.|dept_id`
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ON d.dept_id = e.dept_id",
        "ON d.dept_id = e.__CODEX_CURSOR__dept_id",
    );

    assert!(
        statement.starts_with("CREATE OR REPLACE VIEW"),
        "cursor should stay inside CREATE OR REPLACE VIEW, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinCondition,
        "ON clause inside CREATE OR REPLACE VIEW JOIN should be JoinCondition phase"
    );
    let table_names: Vec<String> = deep_ctx
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
        "alias `e` must be visible inside JOIN ON of CREATE OR REPLACE VIEW: {table_names:?}"
    );
    assert!(
        table_names.iter().any(|n| n == "D"),
        "alias `d` must be visible inside JOIN ON of CREATE OR REPLACE VIEW: {table_names:?}"
    );
}

#[test]
fn mariadb_final_boss_insert_on_duplicate_key_update_values_fn_is_dml_set() {
    // test4.txt: ON DUPLICATE KEY UPDATE with VALUES() references.
    // `ON DUPLICATE KEY UPDATE role_name = VALUES(role_name), ...`
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "role_name = VALUES(role_name),",
        "role_name = VALUES(role_name),\n        __CODEX_CURSOR__",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DmlSetTargetList,
        "ON DUPLICATE KEY UPDATE should produce DmlSetTargetList phase"
    );
    assert!(deep_ctx.phase.is_column_context());
}

#[test]
fn mariadb_final_boss_monster_query_window_function_order_by_is_order_by_clause() {
    // test4.txt Monster query #2: cursor inside a WINDOW function ORDER BY clause.
    // `ROW_NUMBER() OVER (PARTITION BY d.project_id ORDER BY d.|day_hours DESC, d.work_date)`
    // The ORDER BY inside an inline OVER clause sets OrderByClause phase.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ORDER BY d.day_hours DESC,\n                d.work_date",
        "ORDER BY d.__CODEX_CURSOR__day_hours DESC,\n                d.work_date",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause,
        "ORDER BY inside ROW_NUMBER OVER should be OrderByClause phase"
    );
    assert!(
        deep_ctx.phase.is_column_context(),
        "OrderByClause must be a column context"
    );
}

#[test]
fn mariadb_final_boss_recursive_cte_dept_tree_second_member_where() {
    // test4.txt Monster query #2: cursor inside the recursive UNION ALL second member.
    // `FROM departments c JOIN dept_tree t ON t.dept_id = c.|parent_dept_id`
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ON t.dept_id = c.parent_dept_id",
        "ON t.dept_id = c.__CODEX_CURSOR__parent_dept_id",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinCondition,
        "recursive CTE second member ON clause should be JoinCondition phase"
    );
}

#[test]
fn mariadb_final_boss_trigger_body_insert_column_list_is_insert_column_list() {
    // test4.txt: cursor inside INSERT INTO audit_events (...) column list
    // inside the ai_task_log AFTER INSERT trigger body.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "INSERT INTO audit_events (event_type, entity_name, entity_id, detail)",
        "INSERT INTO audit_events (event_type, entity_name, entity_id, __CODEX_CURSOR__detail)",
    );

    assert!(
        statement.starts_with("CREATE TRIGGER ai_task_log"),
        "cursor should stay inside the ai_task_log trigger, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList,
        "INSERT column list inside trigger body should be InsertColumnList phase"
    );
    let table_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        table_names.iter().any(|n| n == "AUDIT_EVENTS"),
        "audit_events must be registered as INSERT target in trigger body, got: {table_names:?}"
    );
}

#[test]
fn mariadb_final_boss_procedure_insert_inside_while_loop_is_insert_column_list() {
    // test4.txt: cursor inside INSERT INTO task_log (...) column list
    // inside the nested WHILE loop of sp_seed_monster_data procedure.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "INSERT INTO task_log (project_id, employee_id, work_date, hours, note, payload)",
        "INSERT INTO task_log (project_id, __CODEX_CURSOR__employee_id, work_date, hours, note, payload)",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_seed_monster_data"),
        "cursor should stay inside sp_seed_monster_data procedure, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList,
        "INSERT column list inside WHILE loop in procedure should be InsertColumnList phase"
    );
    let table_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        table_names.iter().any(|n| n == "TASK_LOG"),
        "task_log must be registered as INSERT target inside WHILE loop, got: {table_names:?}"
    );
}

#[test]
fn mariadb_final_boss_procedure_update_join_set_is_dml_set() {
    // test4.txt: cursor inside SET clause of UPDATE projects p JOIN (...) x ON ... SET p.last_rollup_at
    // in sp_build_monthly_rollup procedure.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "SET p.last_rollup_at = CURRENT_TIMESTAMP(6);",
        "SET p.__CODEX_CURSOR__last_rollup_at = CURRENT_TIMESTAMP(6);",
    );

    assert!(
        statement.starts_with("CREATE PROCEDURE sp_build_monthly_rollup"),
        "cursor should stay inside sp_build_monthly_rollup, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DmlSetTargetList,
        "UPDATE...JOIN...SET inside procedure should be DmlSetTargetList phase"
    );
    assert!(
        deep_ctx.phase.is_column_context(),
        "DmlSetTargetList must be a column context"
    );
    assert!(
        deep_ctx
            .focused_tables
            .iter()
            .any(|t| t.eq_ignore_ascii_case("projects")),
        "focused table for UPDATE...SET should include projects, got: {:?}",
        deep_ctx.focused_tables
    );
}

#[test]
fn mariadb_final_boss_standalone_monster_query1_recursive_cte_select_list() {
    // test4.txt: Monster query #1 is a standalone WITH RECURSIVE... SELECT.
    // Cursor inside the SELECT list of the outer query referencing dept_tree CTE.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    // The outer SELECT references columns from dept_tree CTE
    let (statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "SELECT\n    dept_id,\n    dept_code,\n    dept_name,\n    lvl,\n    path_text\nFROM dept_tree",
        "SELECT\n    __CODEX_CURSOR__dept_id,\n    dept_code,\n    dept_name,\n    lvl,\n    path_text\nFROM dept_tree",
    );

    assert!(
        statement.starts_with("WITH RECURSIVE dept_tree AS"),
        "cursor should be in standalone WITH RECURSIVE statement, got:\n{statement}"
    );
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectList,
        "outer SELECT of WITH RECURSIVE should be SelectList phase"
    );
    let cte_names: Vec<String> = deep_ctx
        .ctes
        .iter()
        .map(|c| c.name.to_uppercase())
        .collect();
    assert!(
        cte_names.iter().any(|n| n == "DEPT_TREE"),
        "DEPT_TREE CTE must be visible in outer SELECT, got: {cte_names:?}"
    );
}

#[test]
fn mariadb_final_boss_monster_query2_owner_chain_cte_dept_tree_visible() {
    // test4.txt Monster query #2: cursor inside owner_chain CTE body.
    // `FROM employees e JOIN dept_tree dt ON dt.dept_id = e.dept_id`
    // The dept_tree CTE defined earlier in the same WITH clause must be visible.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    // This is the 2nd occurrence of `ON t.dept_id = c.parent_dept_id` - but that's different.
    // owner_chain has `ON dt.dept_id = e.dept_id`
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ON dt.dept_id = e.dept_id",
        "ON dt.__CODEX_CURSOR__dept_id = e.dept_id",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinCondition,
        "ON clause inside owner_chain CTE body should be JoinCondition phase"
    );
    let cte_names: Vec<String> = deep_ctx
        .ctes
        .iter()
        .map(|c| c.name.to_uppercase())
        .collect();
    assert!(
        cte_names.iter().any(|n| n == "DEPT_TREE"),
        "DEPT_TREE CTE must be visible inside owner_chain body, got: {cte_names:?}"
    );
    let table_names: Vec<String> = deep_ctx
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
        table_names.iter().any(|n| n == "DT"),
        "alias `dt` (for dept_tree) must be in scope, got: {table_names:?}"
    );
    assert!(
        table_names.iter().any(|n| n == "E"),
        "alias `e` (for employees) must be in scope, got: {table_names:?}"
    );
}

#[test]
fn mariadb_final_boss_monster_query3_json_table_group_by_is_column_context() {
    // test4.txt Monster query #3: WITH tag_usage AS (...FROM task_log t
    // JOIN JSON_TABLE(...) jt GROUP BY p.project_code, jt.tag)
    // Cursor inside the GROUP BY clause, verifying it's GroupByClause phase
    // and that jt (JSON_TABLE alias) is in scope.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "GROUP BY p.project_code,\n        jt.tag",
        "GROUP BY p.project_code,\n        jt.__CODEX_CURSOR__tag",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::GroupByClause,
        "GROUP BY with JSON_TABLE alias should be GroupByClause phase"
    );
    assert!(
        deep_ctx.phase.is_column_context(),
        "GroupByClause must be a column context"
    );
    // jt (JSON_TABLE virtual relation alias) must be visible
    let qualifier_tables =
        crate::ui::intellisense_context::resolve_qualifier_tables("JT", &deep_ctx.tables_in_scope);
    assert!(
        !qualifier_tables.is_empty(),
        "qualifier `jt` (JSON_TABLE alias) must resolve in GROUP BY context, got empty"
    );
}

#[test]
fn mariadb_final_boss_final_inspection_select_from_clause_tables_in_scope() {
    // test4.txt: Final inspection SELECT query (lines ~751-765).
    // Cursor inside WHERE/ORDER BY of the multi-join SELECT to verify all tables visible.
    let script = load_mariadb_intellisense_test_file("test4.txt");
    // Target the ORDER BY clause of the final SELECT
    let (_statement, _cursor, deep_ctx) = analyze_full_script_target_replacement(
        script,
        "ORDER BY mr.ym,\n    p.project_code,\n    e.emp_code;",
        "ORDER BY mr.__CODEX_CURSOR__ym,\n    p.project_code,\n    e.emp_code;",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause,
        "final SELECT ORDER BY should be OrderByClause phase"
    );
    let table_aliases: Vec<String> = deep_ctx
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
        table_aliases.iter().any(|n| n == "MR"),
        "alias `mr` (monthly_rollup) must be in scope, got: {table_aliases:?}"
    );
    assert!(
        table_aliases.iter().any(|n| n == "P"),
        "alias `p` (projects) must be in scope, got: {table_aliases:?}"
    );
    assert!(
        table_aliases.iter().any(|n| n == "E"),
        "alias `e` (employees) must be in scope, got: {table_aliases:?}"
    );
}

#[test]
fn mariadb_scripts_create_table_definition_contexts_do_not_regress_to_table_name() {
    for (file_name, target, replacement) in [
        (
            "test1.txt",
            "order_id BIGINT NOT NULL,",
            "order_id BI__CODEX_CURSOR__ NOT NULL,",
        ),
        (
            "test2.txt",
            "task_id BIGINT NOT NULL,",
            "task_id BI__CODEX_CURSOR__ NOT NULL,",
        ),
        (
            "test3.txt",
            "run_id BIGINT NOT NULL,",
            "run_id BI__CODEX_CURSOR__ NOT NULL,",
        ),
        (
            "test4.txt",
            "dept_id        INT          NOT NULL AUTO_INCREMENT,",
            "dept_id        INT          NOT NULL __CODEX_CURSOR__AUTO_INCREMENT,",
        ),
    ] {
        let script = load_mariadb_intellisense_test_file(file_name);
        let (statement, _cursor, deep_ctx) =
            analyze_full_script_target_replacement(script, target, replacement);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );

        assert!(
            statement.starts_with("CREATE TABLE"),
            "cursor should stay inside CREATE TABLE statement for {file_name}, got:\n{statement}"
        );
        assert_ne!(
            context,
            SqlContext::TableName,
            "CREATE TABLE definition keyword in {file_name} must not stay in table-name context"
        );
    }
}

#[test]
fn mariadb_scripts_create_table_option_contexts_do_not_regress_to_table_name() {
    for (file_name, target, replacement) in [
        (
            "test1.txt",
            ") ENGINE = InnoDB;",
            ") ENG__CODEX_CURSOR__ = InnoDB;",
        ),
        (
            "test2.txt",
            ") ENGINE = InnoDB;",
            ") ENG__CODEX_CURSOR__ = InnoDB;",
        ),
        (
            "test3.txt",
            ") ENGINE = InnoDB;",
            ") ENG__CODEX_CURSOR__ = InnoDB;",
        ),
        (
            "test4.txt",
            ")\nENGINE = InnoDB;",
            ")\nENG__CODEX_CURSOR__ = InnoDB;",
        ),
    ] {
        let script = load_mariadb_intellisense_test_file(file_name);
        let (statement, _cursor, deep_ctx) =
            analyze_full_script_target_replacement(script, target, replacement);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );

        assert!(
            statement.starts_with("CREATE TABLE"),
            "cursor should stay inside CREATE TABLE statement for {file_name}, got:\n{statement}"
        );
        assert_ne!(
            context,
            SqlContext::TableName,
            "CREATE TABLE option keyword in {file_name} must not stay in table-name context"
        );
    }
}

#[test]
fn mysql_create_table_definition_keywords_include_numeric_types_and_nullability() {
    let (bigint_context, bigint_suggestions) =
        mysql_context_and_suggestions_for_inline_sql("CREATE TABLE demo (id BI|)");
    assert_ne!(bigint_context, SqlContext::TableName);
    assert_has_case_insensitive(&bigint_suggestions, "BIGINT");

    let (not_context, not_suggestions) =
        mysql_context_and_suggestions_for_inline_sql("CREATE TABLE demo (id INT NO|)");
    assert_ne!(not_context, SqlContext::TableName);
    assert_has_case_insensitive(&not_suggestions, "NOT");

    let (null_context, null_suggestions) =
        mysql_context_and_suggestions_for_inline_sql("CREATE TABLE demo (id INT NU|)");
    assert_ne!(null_context, SqlContext::TableName);
    assert_has_case_insensitive(&null_suggestions, "NULL");
}

#[test]
fn mysql_create_table_option_keywords_include_engine_default_and_collate() {
    let (engine_context, engine_suggestions) =
        mysql_context_and_suggestions_for_inline_sql("CREATE TABLE demo (id INT) ENG|");
    assert_ne!(engine_context, SqlContext::TableName);
    assert_has_case_insensitive(&engine_suggestions, "ENGINE");

    let (default_context, default_suggestions) = mysql_context_and_suggestions_for_inline_sql(
        "CREATE TABLE demo (id INT) DEF| CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci",
    );
    assert_ne!(default_context, SqlContext::TableName);
    assert_has_case_insensitive(&default_suggestions, "DEFAULT");

    let (collate_context, collate_suggestions) = mysql_context_and_suggestions_for_inline_sql(
        "CREATE TABLE demo (id INT) DEFAULT CHARACTER SET utf8mb4 COL| utf8mb4_unicode_ci",
    );
    assert_ne!(collate_context, SqlContext::TableName);
    assert_has_case_insensitive(&collate_suggestions, "COLLATE");
}

#[test]
fn mysql_lock_in_share_mode_is_not_classified_as_lock_table_context() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM emp LOCK IN SHARE MODE |");
    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_ne!(
        context,
        SqlContext::TableName,
        "LOCK IN SHARE MODE must not switch intellisense back to table-name context"
    );
}

#[test]
fn mysql_straight_join_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
SELECT 1 FROM dual;

SELECT d.__CODEX_CURSOR__deptno
FROM emp e
STRAIGHT_JOIN dept d ON e.deptno = d.deptno
WHERE d.loc = 'SEOUL';
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("STRAIGHT_JOIN dept d"),
        "current statement should stay inside STRAIGHT_JOIN query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("d"), &deep_ctx);
    assert_eq!(tables, vec!["dept".to_string()]);
}

#[test]
fn bracket_quoted_alias_resolution_matches_bracket_qualified_reference() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT [Recent Emp].| FROM emp [Recent Emp]");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("[Recent Emp]"), &deep_ctx);
    assert_eq!(tables, vec!["emp".to_string()]);
}

#[test]
fn bracket_quoted_alias_resolution_unescapes_closing_brackets() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT [Recent]]Emp].| FROM emp [Recent]]Emp]");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables =
        SqlEditorWidget::resolve_column_tables_for_context(Some("[Recent]]Emp]"), &deep_ctx);
    assert_eq!(tables, vec!["emp".to_string()]);
}

#[test]
fn mysql_use_index_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
SELECT 'warmup';

SELECT o.order_id
FROM orders USE INDEX (idx_orders_date) o
JOIN customers c ON c.id = o.customer_id
WHERE c.__CODEX_CURSOR__status = 'A';
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("USE INDEX (idx_orders_date) o"),
        "current statement should stay inside USE INDEX query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::WhereClause);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("c"), &deep_ctx);
    assert_eq!(tables, vec!["customers".to_string()]);
}

#[test]
fn mysql_force_index_for_order_by_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
SELECT 'warmup';

SELECT o.__CODEX_CURSOR__order_id
FROM orders FORCE INDEX FOR ORDER BY (idx_orders_date) o
WHERE o.created_at >= CURRENT_DATE - INTERVAL '1' DAY;
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("FORCE INDEX FOR ORDER BY"),
        "current statement should stay inside FORCE INDEX query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("o"), &deep_ctx);
    assert_eq!(tables, vec!["orders".to_string()]);
}

#[test]
fn oracle_partition_clause_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
PROMPT partition check

SELECT s.__CODEX_CURSOR__amount
FROM sales PARTITION (p202401) s
WHERE s.region_id = 1;
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("PARTITION (p202401) s"),
        "current statement should stay inside PARTITION query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("s"), &deep_ctx);
    assert_eq!(tables, vec!["sales".to_string()]);
}

#[test]
fn oracle_tablesample_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
PROMPT tablesample check

SELECT s.__CODEX_CURSOR__amount
FROM sales TABLESAMPLE BERNOULLI (10) REPEATABLE (7) s
WHERE s.region_id = 1;
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("TABLESAMPLE BERNOULLI (10) REPEATABLE (7) s"),
        "current statement should stay inside TABLESAMPLE query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("s"), &deep_ctx);
    assert_eq!(tables, vec!["sales".to_string()]);
}

#[test]
fn oracle_partitioned_outer_join_alias_resolution_survives_full_script_statement_slicing() {
    let script = "\
SELECT 'warmup' FROM dual;

SELECT t.__CODEX_CURSOR__region_id
FROM sales s PARTITION BY (s.region_id)
RIGHT OUTER JOIN targets t ON s.region_id = t.region_id
WHERE t.region_id IS NOT NULL;
";

    let (statement, _cursor, deep_ctx) = analyze_full_script_marker(script);
    assert!(
        statement.contains("PARTITION BY (s.region_id)"),
        "current statement should stay inside partitioned outer join query, got:\n{statement}"
    );
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("t"), &deep_ctx);
    assert_eq!(tables, vec!["targets".to_string()]);
}

#[test]
fn statement_bounds_slash_terminates_create_plsql_block() {
    // After 'CREATE FUNCTION ... IS BEGIN ... END;\n/\n', a subsequent
    // SELECT should be recognised as a separate statement.
    let sql = "\
CREATE OR REPLACE FUNCTION oqt_f_add(p_a NUMBER, p_b NUMBER)\nRETURN NUMBER\nIS\nBEGIN\n  RETURN NVL(p_a,0) + NVL(p_b,0);\nEND;\n/\nSELECT empno FROM oqt_emp;";
    let cursor = sql.find("empno FROM").unwrap();
    let (start, end) = SqlEditorWidget::statement_bounds_in_text(sql, cursor);
    let stmt = sql.get(start..end).unwrap_or("");
    assert!(
        stmt.contains("SELECT empno FROM oqt_emp"),
        "expected SELECT statement, got: {:?}",
        stmt
    );
    assert!(
        !stmt.contains("CREATE"),
        "CREATE should not leak into the SELECT statement: {:?}",
        stmt
    );
}

#[test]
fn statement_bounds_multiple_create_blocks_with_slash() {
    // Multiple CREATE blocks terminated by '/' followed by a SELECT
    let sql = "\
CREATE OR REPLACE FUNCTION f1 RETURN NUMBER IS\nBEGIN\n  RETURN 1;\nEND;\n/\n\
CREATE OR REPLACE PROCEDURE p1 IS\nBEGIN\n  NULL;\nEND;\n/\n\
SELECT sa FROM oqt_emp ORDER BY empno;";
    let cursor = sql.find("sa FROM").unwrap();
    let (start, end) = SqlEditorWidget::statement_bounds_in_text(sql, cursor);
    let stmt = sql.get(start..end).unwrap_or("");
    assert!(
        stmt.starts_with("SELECT") || stmt.trim_start().starts_with("SELECT"),
        "expected SELECT statement, got: {:?}",
        stmt
    );
    assert!(
        stmt.contains("oqt_emp"),
        "expected oqt_emp in statement: {:?}",
        stmt
    );
}

#[test]
fn statement_bounds_script_with_plsql_blocks_then_select() {
    // Simulates a realistic script: anonymous PL/SQL blocks, CREATE blocks,
    // followed by a SELECT at the end. The cursor is inside the final SELECT.
    let sql = "\
BEGIN\n  EXECUTE IMMEDIATE 'DROP TABLE oqt_emp PURGE';\nEXCEPTION WHEN OTHERS THEN NULL;\nEND;\n/\n\
CREATE TABLE oqt_emp (\n  empno NUMBER PRIMARY KEY,\n  ename VARCHAR2(50),\n  salary NUMBER\n);\n\
INSERT INTO oqt_emp(empno, ename, salary) VALUES (100, 'ALICE', 3000);\nCOMMIT;\n\
CREATE OR REPLACE FUNCTION oqt_f_add(p_a NUMBER, p_b NUMBER)\nRETURN NUMBER\nIS\nBEGIN\n  RETURN NVL(p_a,0) + NVL(p_b,0);\nEND;\n/\n\
PROMPT === final ===\n\
SELECT empno, ename, sa FROM oqt_emp ORDER BY empno;";

    let cursor = sql.find("sa FROM oqt_emp").unwrap();
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(sql, cursor);
    let stmt = sql.get(stmt_start..stmt_end).unwrap_or("");
    assert!(
        stmt.contains("oqt_emp"),
        "statement should contain oqt_emp: {:?}",
        stmt
    );
    assert!(
        stmt.contains("SELECT"),
        "statement should contain SELECT: {:?}",
        stmt
    );

    // Now test context analysis for intellisense
    let context_text = SqlEditorWidget::normalize_intellisense_context_text(
        sql.get(stmt_start..cursor).unwrap_or(""),
    );
    let statement_text = SqlEditorWidget::normalize_intellisense_context_text(
        sql.get(stmt_start..stmt_end).unwrap_or(""),
    );

    let token_spans = super::query_text::tokenize_sql_spanned(&statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= context_text.len());
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectList,
        "cursor should be in SelectList phase"
    );

    let table_names: Vec<String> = deep_ctx
        .tables_in_scope
        .iter()
        .map(|t| t.name.to_uppercase())
        .collect();
    assert!(
        table_names.contains(&"OQT_EMP".to_string()),
        "oqt_emp should be in scope: {:?}",
        table_names
    );
}

#[test]
fn qualifier_before_word_supports_quoted_identifier() {
    let sql_with_cursor = r#"SELECT "e".| FROM "Emp Table" "e""#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("e"));
}

#[test]
fn qualifier_before_word_supports_backtick_quoted_identifier() {
    let sql_with_cursor = "SELECT `e`.| FROM `Emp Table` `e`";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("e"));
}

#[test]
fn qualifier_before_word_supports_bracket_quoted_identifier() {
    let sql_with_cursor = "SELECT [Recent Emp].| FROM emp [Recent Emp]";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    let raw_qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, cursor);

    assert_eq!(qualifier.as_deref(), Some("Recent Emp"));
    assert_eq!(raw_qualifier.as_deref(), Some("[Recent Emp]"));
}

#[test]
fn qualifier_before_word_supports_escaped_bracket_quoted_identifier() {
    let sql_with_cursor = "SELECT [Recent]]Emp].| FROM emp [Recent]]Emp]";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    let raw_qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, cursor);

    assert_eq!(qualifier.as_deref(), Some("Recent]Emp"));
    assert_eq!(raw_qualifier.as_deref(), Some("[Recent]]Emp]"));
}

#[test]
fn qualifier_before_word_rejects_whitespace_between_dot_and_cursor() {
    let sql_with_cursor = "SELECT e.   | FROM emp e";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_rejects_whitespace_before_dot() {
    let sql_with_cursor = "SELECT e   .| FROM emp e";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_rejects_whitespace_before_dot_with_quoted_identifier() {
    let sql_with_cursor = r#"SELECT "e"   .| FROM "Emp Table" "e""#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_supports_unicode_identifier() {
    let sql_with_cursor = "SELECT 사용자.| FROM emp 사용자";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("사용자"));
}

#[test]
fn qualifier_before_word_supports_multi_part_qualifier_chain() {
    let sql_with_cursor = "SELECT schema_a.emp.| FROM schema_a.emp";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("schema_a.emp"));
}

#[test]
fn qualifier_before_word_supports_multi_part_qualifier_chain_with_quotes() {
    let sql_with_cursor = r#"SELECT "schema A"."Emp Table".| FROM "schema A"."Emp Table""#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("schema A.Emp Table"));
}

#[test]
fn qualifier_before_word_supports_multi_part_qualifier_chain_with_backticks() {
    let sql_with_cursor = "SELECT `schema A`.`Emp Table`.| FROM `schema A`.`Emp Table`";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("schema A.Emp Table"));
}

#[test]
fn qualifier_before_word_supports_indexed_record_expression() {
    let sql_with_cursor = "BEGIN v_emps(1).| := 1; END;";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);

    assert_eq!(qualifier.as_deref(), Some("v_emps"));
}

#[test]
fn qualifier_before_word_supports_indexed_record_expression_with_dotted_string_key() {
    let sql_with_cursor = "BEGIN v_emps('HOME.WORK').| := 1; END;";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    let raw_qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, cursor);

    assert_eq!(qualifier.as_deref(), Some("v_emps"));
    assert_eq!(raw_qualifier.as_deref(), Some("v_emps('HOME.WORK')"));
}

#[test]
fn qualifier_before_word_supports_quoted_record_field_with_dot() {
    let sql_with_cursor = r#"BEGIN v_emp."Addr.Info".| := 1; END;"#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    let raw_qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, cursor);

    assert_eq!(qualifier.as_deref(), Some("v_emp.Addr.Info"));
    assert_eq!(raw_qualifier.as_deref(), Some(r#"v_emp."Addr.Info""#));
}

#[test]
fn raw_qualifier_before_word_preserves_quoted_identifier_text() {
    let sql_with_cursor = r#"SELECT "schema A"."Emp Table"."Column X"| FROM dual"#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let (_word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(&sql, cursor)
        .expect("quoted identifier should be resolved at cursor");
    let qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, start);
    assert_eq!(qualifier.as_deref(), Some(r#""schema A"."Emp Table""#));
}

#[test]
fn raw_qualifier_before_word_preserves_backtick_identifier_text() {
    let sql_with_cursor = "SELECT `schema A`.`Emp Table`.`Column X`| FROM dual";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let (_word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(&sql, cursor)
        .expect("backtick identifier should be resolved at cursor");
    let qualifier = SqlEditorWidget::raw_qualifier_before_word_in_text(&sql, start);
    assert_eq!(qualifier.as_deref(), Some("`schema A`.`Emp Table`"));
}

#[test]
fn object_context_reference_at_position_includes_clicked_identifier_qualifier() {
    let sql = "begin\n  demo_pkg.run_job;\nend;";
    let pos = sql.find("run_job").unwrap() as i32 + 2;
    let line_start = sql.find("demo_pkg").unwrap();
    let line_end = sql.find(';').unwrap();
    let line = &sql[line_start..line_end];

    let (reference, start, end) =
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos as usize - line_start)
            .expect("object reference should resolve");

    assert_eq!(reference, "demo_pkg.run_job");
    assert_eq!(&line[start..end], "run_job");
}

#[test]
fn object_context_reference_at_position_ignores_table_alias_declaration() {
    let line = "SELECT * FROM all_objects a JOIN all_objects b ON b.object_id = a.object_id";
    let pos = line.find("all_objects a").unwrap() + "all_objects ".len();

    assert!(
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos).is_none(),
        "right-clicking alias declaration `a` should not create an object context candidate"
    );
}

#[test]
fn object_context_reference_at_position_ignores_bracket_table_alias_declaration() {
    let line = "SELECT * FROM all_objects [Recent Emp]";
    let pos = line.find("[Recent Emp]").unwrap() + "[Recent ".len();

    assert!(
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos).is_none(),
        "right-clicking bracket alias declaration should not create an object context candidate"
    );
}

#[test]
fn object_context_reference_at_position_ignores_escaped_bracket_table_alias_declaration() {
    let line = "SELECT * FROM all_objects [Recent]]Emp]";
    let pos = line.find("[Recent]]Emp]").unwrap() + "[Recent]]".len();

    assert!(
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos).is_none(),
        "right-clicking escaped bracket alias declaration should not create an object context candidate"
    );
}

#[test]
fn object_context_reference_at_position_ignores_alias_qualifier() {
    let line = "SELECT a.object_id, b.object_id FROM all_objects a JOIN all_objects b ON b.object_id = a.object_id";
    let pos = line.find("a.object_id").unwrap();

    assert!(
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos).is_none(),
        "right-clicking alias qualifier `a` should not create an object context candidate"
    );
}

#[test]
fn object_context_reference_at_position_keeps_clicked_object_before_alias() {
    let line = "SELECT * FROM all_objects a";
    let pos = line.find("all_objects").unwrap() + 2;

    let (reference, start, end) =
        SqlEditorWidget::object_context_reference_at_position_in_text(line, pos)
            .expect("table object should still resolve before its alias");

    assert_eq!(reference, "all_objects");
    assert_eq!(&line[start..end], "all_objects");
}

#[test]
fn right_click_object_context_candidates_try_clicked_reference_before_selection() {
    let candidates = SqlEditorWidget::right_click_object_context_candidates(
        Some("demo_pkg.run_job"),
        "select * from demo_pkg.run_job",
    );

    assert_eq!(
        candidates,
        vec![
            "demo_pkg.run_job".to_string(),
            "select * from demo_pkg.run_job".to_string()
        ]
    );
}

#[test]
fn right_click_object_context_candidates_fall_back_to_selection() {
    let candidates =
        SqlEditorWidget::right_click_object_context_candidates(None, "demo_pkg.run_job");

    assert_eq!(candidates, vec!["demo_pkg.run_job".to_string()]);
}

#[test]
fn identifier_at_position_supports_unicode_identifier() {
    let sql = "SELECT 사용자 FROM dual";
    let cursor = sql.find("사용자").unwrap_or(0) + "사용자".len();

    let (word, start, end) = SqlEditorWidget::identifier_at_position_in_text(sql, cursor)
        .expect("unicode identifier should be resolved at cursor");
    assert_eq!(word, "사용자");
    assert_eq!(sql.get(start..end), Some("사용자"));
}

#[test]
fn identifier_at_position_supports_quoted_unicode_identifier() {
    let sql = r#"SELECT "사용자"."이름" FROM dual"#;
    let cursor = sql.find(r#""이름""#).unwrap_or(0) + r#""이름""#.len();

    let (word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(sql, cursor)
        .expect("quoted unicode identifier should be resolved at cursor");
    assert_eq!(word, "이름");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(sql, start);
    assert_eq!(qualifier.as_deref(), Some("사용자"));
}

#[test]
fn identifier_at_position_supports_backtick_quoted_identifier() {
    let sql = "SELECT `사용자`.`이름` FROM dual";
    let cursor = sql.find("`이름`").unwrap_or(0) + "`이름`".len();

    let (word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(sql, cursor)
        .expect("backtick-quoted identifier should be resolved at cursor");
    assert_eq!(word, "이름");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(sql, start);
    assert_eq!(qualifier.as_deref(), Some("사용자"));
}

#[test]
fn identifier_at_position_supports_bracket_quoted_identifier() {
    let sql = "SELECT [사용자].[이름] FROM dual";
    let cursor = sql.find("[이름]").unwrap_or(0) + "[이름]".len();

    let (word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(sql, cursor)
        .expect("bracket-quoted identifier should be resolved at cursor");
    assert_eq!(word, "이름");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(sql, start);
    assert_eq!(qualifier.as_deref(), Some("사용자"));
}

#[test]
fn identifier_at_position_supports_escaped_bracket_quoted_identifier() {
    let sql = "SELECT [사용자]].부서].[이름]]값] FROM dual";
    let cursor = sql.find("[이름]]값]").unwrap_or(0) + "[이름]]값]".len();

    let (word, start, _end) = SqlEditorWidget::identifier_at_position_in_text(sql, cursor)
        .expect("escaped bracket-quoted identifier should be resolved at cursor");
    assert_eq!(word, "이름]값");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(sql, start);
    assert_eq!(qualifier.as_deref(), Some("사용자].부서"));
}

#[test]
fn qualifier_before_word_rejects_numeric_identifier_start() {
    let sql_with_cursor = "SELECT 1emp.| FROM emp";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_allows_special_identifier_start_chars() {
    let sql_with_cursor = "SELECT _emp.| FROM emp _emp";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("_emp"));
}

#[test]
fn quick_describe_lookup_name_uses_current_schema_for_unqualified_object() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", None, Some("HR")),
        "HR.EMP"
    );
}

#[test]
fn quick_describe_lookup_name_preserves_explicit_qualifier() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", Some("scott"), Some("hr")),
        "SCOTT.EMP"
    );
}

#[test]
fn quick_describe_lookup_name_quotes_noncanonical_current_schema() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", None, Some("Sales Ops")),
        r#""Sales Ops".EMP"#
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", None, Some("SalesOps")),
        r#""SalesOps".EMP"#
    );
}

#[test]
fn quick_describe_lookup_name_preserves_quoted_parts() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(
            r#""Emp Table""#,
            Some(r#""Sales Ops""#),
            Some("HR")
        ),
        r#""Sales Ops"."Emp Table""#
    );
}

#[test]
fn quick_describe_lookup_name_converts_backtick_parts_for_oracle_lookup() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("`Emp Table`", Some("`Sales Ops`"), Some("HR")),
        r#""Sales Ops"."Emp Table""#
    );
}

#[test]
fn quick_describe_lookup_name_converts_bracket_parts_for_oracle_lookup() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("[Emp Table]", Some("[Sales Ops]"), Some("HR")),
        r#""Sales Ops"."Emp Table""#
    );
}

#[test]
fn quick_describe_lookup_name_preserves_bracket_dotted_parts() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("[Emp.Table]", Some("[Sales.Ops]"), Some("HR")),
        r#""Sales.Ops"."Emp.Table""#
    );
}

#[test]
fn quick_describe_lookup_name_unescapes_quoted_identifier_delimiters() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(
            r#""Emp""Name""#,
            Some(r#""Sales""Ops""#),
            Some("HR")
        ),
        r#""Sales""Ops"."Emp""Name""#
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(
            "`Emp``Name`",
            Some("`Sales``Ops`"),
            Some("HR")
        ),
        r#""Sales`Ops"."Emp`Name""#
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(
            "[Emp]]Name]",
            Some("[Sales]]Ops]"),
            Some("HR")
        ),
        r#""Sales]Ops"."Emp]Name""#
    );
}

#[test]
fn quick_describe_lookup_name_rejects_malformed_quoted_identifier() {
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(r#""emp"#, None, Some("HR")),
        ""
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", Some(r#""bad.schema"#), Some("HR")),
        ""
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name(r#"bad"name"#, None, Some("HR")),
        ""
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", Some(r#"bad"schema"#), Some("HR")),
        ""
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("[emp", None, Some("HR")),
        ""
    );
    assert_eq!(
        SqlEditorWidget::quick_describe_lookup_name("emp", Some("[bad.schema"), Some("HR")),
        ""
    );
}

#[test]
fn quick_describe_package_lookup_names_use_current_schema_for_unqualified_package() {
    assert_eq!(
        SqlEditorWidget::quick_describe_package_lookup_names(Some("demo_pkg"), Some("HR")),
        vec!["HR.DEMO_PKG".to_string()]
    );
}

#[test]
fn quick_describe_package_lookup_names_fall_back_to_bare_when_current_schema_unknown() {
    assert_eq!(
        SqlEditorWidget::quick_describe_package_lookup_names(Some("demo_pkg"), None),
        vec!["DEMO_PKG".to_string()]
    );
}

#[test]
fn quick_describe_package_lookup_names_keep_explicit_owner() {
    assert_eq!(
        SqlEditorWidget::quick_describe_package_lookup_names(Some("scott.demo_pkg"), Some("hr")),
        vec!["SCOTT.DEMO_PKG".to_string()]
    );
}

#[test]
fn quick_describe_package_lookup_names_preserve_quoted_owner() {
    assert_eq!(
        SqlEditorWidget::quick_describe_package_lookup_names(
            Some(r#""Sales Ops"."Demo Pkg""#),
            Some("HR")
        ),
        vec![r#""Sales Ops"."Demo Pkg""#.to_string()]
    );
}

#[test]
fn quick_describe_package_lookup_names_do_not_treat_quoted_dot_as_owner_separator() {
    assert_eq!(
        SqlEditorWidget::quick_describe_package_lookup_names(Some(r#""Demo.Pkg""#), Some("HR")),
        vec![r#"HR."Demo.Pkg""#.to_string()]
    );
}

#[test]
fn normalize_intellisense_context_text_skips_leading_prompt_lines() {
    let input = "PROMPT [3] WITH basic + note\n-- separator\nWITH cte AS (SELECT 1 FROM dual)\nSELECT * FROM cte";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert!(normalized.starts_with("WITH cte AS"));
    assert!(!normalized.starts_with("PROMPT"));
}

#[test]
fn normalize_intellisense_context_text_strips_sqlplus_line_prefixes() {
    let input = "SQL> WITH cte AS (SELECT 1 FROM dual)
  2  SELECT * FROM cte
";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(
        normalized,
        "WITH cte AS (SELECT 1 FROM dual)
SELECT * FROM cte
"
    );
}

#[test]
fn normalize_intellisense_context_text_strips_unindented_sqlplus_numbered_prefixes() {
    let input = "SQL> SELECT e.
2  FROM emp e
";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(
        normalized,
        "SELECT e.
FROM emp e
"
    );
}

#[test]
fn normalize_intellisense_context_with_cursor_maps_unindented_numbered_prefixes() {
    let raw = "SQL> SELECT e.
2  FROM emp e
";
    let raw_cursor = raw.find("e.").unwrap_or(0) + 2;
    let (normalized, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(raw, raw_cursor);

    assert_eq!(
        normalized,
        "SELECT e.
FROM emp e
"
    );
    assert_eq!(
        normalized.get(..normalized_cursor).unwrap_or(""),
        "SELECT e."
    );
}

#[test]
fn normalize_intellisense_context_text_strips_unindented_sqlplus_line_prefixes() {
    let input = "SQL> SELECT e.\n2  FROM emp e\n";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(normalized, "SELECT e.\nFROM emp e\n");
}

#[test]
fn normalize_intellisense_context_with_cursor_maps_unindented_sqlplus_line_prefixes() {
    let raw = "SQL> SELECT e.\n2  FROM emp e\n";
    let raw_cursor = raw.find("e.").unwrap_or(0) + 2;
    let (normalized, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(raw, raw_cursor);

    assert_eq!(normalized, "SELECT e.\nFROM emp e\n");
    assert_eq!(
        normalized.get(..normalized_cursor).unwrap_or(""),
        "SELECT e."
    );
}

#[test]
fn normalize_intellisense_context_text_keeps_numeric_literal_line_prefixes() {
    let input = "SELECT\n1 + 2 AS total\nFROM dual";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(normalized, input);
}

#[test]
fn normalize_intellisense_context_text_keeps_unindented_numeric_lines_with_wide_spacing() {
    let input = "SELECT\n1  + 2 AS total\nFROM dual";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(normalized, input);
}

#[test]
fn normalize_intellisense_context_text_keeps_indented_numeric_lines_without_sql_prompt() {
    let input = "SELECT\n  1  + 2 AS total\nFROM dual";
    let normalized = SqlEditorWidget::normalize_intellisense_context_text(input);

    assert_eq!(normalized, input);
}

#[test]
fn normalize_intellisense_context_with_cursor_maps_byte_offset_after_prompt_stripping() {
    let raw = "PROMPT header\nSQL> SELECT e.\n  2  FROM emp e\n";
    let raw_cursor = raw.find("e.").expect("cursor anchor should exist") + 2;
    let (normalized, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(raw, raw_cursor);

    assert_eq!(normalized, "SELECT e.\nFROM emp e\n");
    assert_eq!(&normalized[..normalized_cursor], "SELECT e.");

    let full_token_spans = super::query_text::tokenize_sql_spanned(&normalized);
    let split_idx = full_token_spans.partition_point(|span| span.end <= normalized_cursor);
    let full_tokens: Vec<SqlToken> = full_token_spans
        .into_iter()
        .map(|span| span.token)
        .collect();
    let ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);
    assert_eq!(ctx.phase, intellisense_context::SqlPhase::SelectList);
    assert!(
        ctx.tables_in_scope
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case("emp")),
        "emp should remain visible after byte-offset remapping"
    );
}

#[test]
fn normalize_intellisense_context_with_cursor_maps_offset_for_unindented_numbered_lines() {
    let raw = "SQL> SELECT e.\n2  FROM emp e\n";
    let raw_cursor = raw.find("e.").expect("cursor anchor should exist") + 2;
    let (normalized, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(raw, raw_cursor);

    assert_eq!(normalized, "SELECT e.\nFROM emp e\n");
    assert_eq!(&normalized[..normalized_cursor], "SELECT e.");
}

#[test]
fn normalize_intellisense_context_text_matches_cursor_variant_at_end() {
    let raw = "PROMPT header\nSQL> -- skip me\nSQL> SELECT 한글.\n  2  FROM emp 한글\n";
    let normalized_text = SqlEditorWidget::normalize_intellisense_context_text(raw);
    let (normalized_with_cursor, normalized_cursor) =
        SqlEditorWidget::normalize_intellisense_context_with_cursor(raw, raw.len());

    assert_eq!(normalized_with_cursor, normalized_text);
    assert_eq!(
        normalized_with_cursor
            .get(..normalized_cursor)
            .unwrap_or(""),
        "SELECT 한글.\nFROM emp 한글"
    );
}

#[test]
fn prompt_line_before_with_does_not_break_cte_qualified_column_resolution() {
    let sql_with_cursor = r#"
PROMPT [3] WITH basic + multiple CTE + join + scalar subquery + nested expressions
WITH
  d AS (
    SELECT deptno, dname, loc
    FROM oqt_t_dept
  )
SELECT d.|, d.loc
FROM d
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let context_text =
        SqlEditorWidget::normalize_intellisense_context_text(sql.get(..cursor).unwrap_or(""));
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = SqlEditorWidget::normalize_intellisense_context_text(
        sql.get(stmt_start..stmt_end).unwrap_or(""),
    );

    let token_spans = super::query_text::tokenize_sql_spanned(&statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= context_text.len());
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("d")),
        "expected CTE d in parsed context: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let column_tables =
        intellisense_context::resolve_qualifier_tables("d", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["d".to_string()]);

    let mut data = IntellisenseData::new();
    for cte in &deep_ctx.ctes {
        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            cte.body_range,
        );
        let mut columns = if !cte.explicit_columns.is_empty() {
            cte.explicit_columns.clone()
        } else if !cte.body_range.is_empty() {
            intellisense_context::extract_select_list_columns(body_tokens)
        } else {
            Vec::new()
        };
        SqlEditorWidget::dedup_column_names_case_insensitive(&mut columns);
        if !columns.is_empty() {
            data.set_virtual_table_columns(&cte.name, columns);
        }
    }

    let suggestions = data.get_column_suggestions("", Some(&column_tables));
    assert!(
        suggestions
            .iter()
            .any(|col| col.eq_ignore_ascii_case("DNAME")),
        "expected DNAME suggestion for d.* scope, got: {:?}",
        suggestions
    );
}

#[test]
fn future_cte_does_not_pollute_earlier_cte_body_virtual_columns() {
    let sql_with_cursor =
            "WITH c1 AS (SELECT __CODEX_CURSOR__1 AS id FROM dual), c2 AS (SELECT 2 AS id FROM dual) SELECT * FROM c1";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .all(|cte| !cte.name.eq_ignore_ascii_case("c2")),
        "future sibling CTE must not be visible while completing inside an earlier CTE body: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_table_columns
            .keys()
            .all(|name| !name.eq_ignore_ascii_case("c2")),
        "future sibling CTE columns must not enter completion cache for an earlier CTE body: {:?}",
        virtual_table_columns
    );
}

#[test]
fn with_function_followed_by_cte_keeps_virtual_columns_visible() {
    let sql_with_cursor = "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; \
             recursive_tree AS (SELECT 1 AS id FROM dual) \
             SELECT recursive_tree.__CODEX_CURSOR__id FROM recursive_tree";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("recursive_tree")),
        "CTE after WITH FUNCTION should remain available for completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "recursive_tree")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "CTE columns after WITH FUNCTION should stay available for completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn with_function_nested_declare_block_keeps_virtual_columns_visible() {
    let sql_with_cursor = r#"WITH FUNCTION calc_depth RETURN NUMBER IS
BEGIN
    DECLARE
        v_depth NUMBER := 1;
    BEGIN
        v_depth := v_depth + 1;
    END;
    RETURN v_depth;
END;
recursive_tree AS (SELECT 1 AS id FROM dual)
SELECT recursive_tree.__CODEX_CURSOR__id FROM recursive_tree"#;
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("recursive_tree")),
        "CTE after WITH FUNCTION nested DECLARE block should remain available for completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "recursive_tree")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "CTE columns after WITH FUNCTION nested DECLARE block should stay available for completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn with_function_followed_by_explicit_with_keeps_virtual_columns_visible() {
    let sql_with_cursor = "WITH FUNCTION calc_depth RETURN NUMBER IS BEGIN RETURN 1; END; \
             WITH recursive_tree AS (SELECT 1 AS id FROM dual) \
             SELECT recursive_tree.__CODEX_CURSOR__id FROM recursive_tree";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("recursive_tree")),
        "explicit WITH after WITH FUNCTION should remain available for completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "recursive_tree")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "explicit WITH CTE columns after WITH FUNCTION should stay available for completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn insert_with_cte_source_query_keeps_virtual_columns_visible() {
    let sql_with_cursor = "INSERT INTO audit_log WITH recent AS (SELECT 1 AS id FROM dual) \
             SELECT recent.__CODEX_CURSOR__id FROM recent";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("recent")),
        "insert-source WITH should remain available for completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "recent")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "insert-source CTE columns should stay available for completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn recursive_cte_body_keeps_virtual_columns_visible() {
    let sql_with_cursor =
            "WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT r.__CODEX_CURSOR__n FROM r WHERE n < 10) \
             SELECT * FROM r";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("r")),
        "recursive CTE should remain available inside its own body: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "r")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("n")),
        "recursive CTE columns should stay available inside its own body: {:?}",
        virtual_table_columns
    );
}

#[test]
fn non_recursive_cte_body_does_not_expose_self_virtual_columns() {
    let sql_with_cursor =
        "WITH temp AS (SELECT temp.__CODEX_CURSOR__id FROM users) SELECT * FROM temp";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .all(|cte| !cte.name.eq_ignore_ascii_case("temp")),
        "non-recursive CTE must not be visible inside its own body: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        deep_ctx
            .tables_in_scope
            .iter()
            .all(|table| !table.name.eq_ignore_ascii_case("temp")),
        "non-recursive CTE must stay out of visible table scope inside its own body: {:?}",
        deep_ctx.tables_in_scope
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_table_columns
            .keys()
            .all(|name| !name.eq_ignore_ascii_case("temp")),
        "non-recursive CTE must not populate virtual columns inside its own body: {:?}",
        virtual_table_columns
    );
}

#[test]
fn outer_cte_in_nested_from_subquery_keeps_virtual_columns_visible() {
    let sql_with_cursor = "WITH outer_cte AS (SELECT 1 AS id FROM dual) \
             SELECT * FROM (SELECT outer_cte.__CODEX_CURSOR__id FROM outer_cte) sub";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("outer_cte")),
        "outer CTE should remain available inside nested FROM subquery completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "outer_cte")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "outer CTE columns should stay available inside nested FROM subquery completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn outer_cte_in_second_set_operator_operand_keeps_virtual_columns_visible() {
    let sql_with_cursor = "WITH outer_cte AS (SELECT 1 AS id FROM dual) \
             SELECT id FROM outer_cte UNION ALL SELECT outer_cte.__CODEX_CURSOR__id FROM outer_cte";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    assert!(
        deep_ctx
            .ctes
            .iter()
            .any(|cte| cte.name.eq_ignore_ascii_case("outer_cte")),
        "outer CTE should remain available in later set-operator operand completion: {:?}",
        deep_ctx
            .ctes
            .iter()
            .map(|cte| cte.name.clone())
            .collect::<Vec<_>>()
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    assert!(
        virtual_columns_for(&virtual_table_columns, "outer_cte")
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id")),
        "outer CTE columns should stay available in later set-operator operand completion: {:?}",
        virtual_table_columns
    );
}

#[test]
fn lateral_subquery_star_virtual_columns_exclude_outer_scope_columns() {
    let sql_with_cursor = "SELECT src.__CODEX_CURSOR__id \
         FROM parent_table p \
         CROSS APPLY (SELECT * FROM child_table c WHERE c.parent_id = p.id) src";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(sql_with_cursor);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "PARENT_TABLE",
            vec!["ID".to_string(), "PARENT_ONLY".to_string()],
        );
        guard.set_columns_for_table(
            "CHILD_TABLE",
            vec![
                "ID".to_string(),
                "PARENT_ID".to_string(),
                "CHILD_ONLY".to_string(),
            ],
        );
    }

    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);

    let columns = virtual_columns_for(&virtual_table_columns, "src").clone();
    assert_has_case_insensitive(&columns, "ID");
    assert_has_case_insensitive(&columns, "PARENT_ID");
    assert_has_case_insensitive(&columns, "CHILD_ONLY");
    assert!(
        !columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case("PARENT_ONLY")),
        "correlated subquery wildcard should not pull outer-scope columns: {:?}",
        columns
    );
}

#[test]
fn statement_context_uses_window_slice_for_large_multiline_statement() {
    let mut sql = String::from("SELECT\n");
    for _ in 0..3_000 {
        sql.push_str("col_a, col_b, col_c, col_d, col_e, col_f, col_g,\n");
    }
    sql.push_str("dummy_table.col_h,\n");
    sql.push_str("dummy_table.col_i\n");
    sql.push_str("FROM dummy_schema.dummy_table\n");

    let cursor = sql.len();
    let context = SqlEditorWidget::statement_context_in_text(&sql, cursor);
    assert!(
        context.contains("dummy_table.col_h"),
        "statement_context should include the latest select list columns, got {:?}",
        context.get(0..120).unwrap_or("")
    );
}

#[test]
fn context_before_cursor_uses_window_slice_for_large_multiline_statement() {
    let mut sql = String::from("SELECT\n");
    for _ in 0..3_000 {
        sql.push_str("col_a, col_b, col_c, col_d, col_e, col_f, col_g,\n");
    }
    sql.push_str("dummy_table.col_h,\n");
    sql.push_str("dummy_table.col_i\n");
    sql.push_str("FROM dummy_schema.dummy_table\n");

    let cursor = sql.len();
    let context = SqlEditorWidget::context_before_cursor_in_text(&sql, cursor);
    assert!(
        context.contains("dummy_table.col_i"),
        "context_before_cursor should include the latest select list columns, got {:?}",
        context.get(0..120).unwrap_or("")
    );
}

#[test]
fn statement_context_window_clamps_utf8_start_boundary() {
    let mut sql = String::from("가");
    sql.push_str(&"a".repeat(INTELLISENSE_STATEMENT_WINDOW as usize - 1));
    let cursor = sql.len();

    let context = SqlEditorWidget::statement_context_in_text(&sql, cursor);
    assert!(
        !context.is_empty(),
        "statement_context should not become empty when window starts in UTF-8 middle byte"
    );
    assert!(context.contains('가'));
}

#[test]
fn context_before_cursor_window_clamps_utf8_start_boundary() {
    let mut sql = String::from("가");
    sql.push_str(&"a".repeat(INTELLISENSE_CONTEXT_WINDOW as usize - 1));
    let cursor = sql.len();

    let context = SqlEditorWidget::context_before_cursor_in_text(&sql, cursor);
    assert!(
        !context.is_empty(),
        "context_before_cursor should not become empty when window starts in UTF-8 middle byte"
    );
    assert!(context.contains('가'));
}

#[test]
fn qualifier_before_word_in_text_supports_quoted_identifier_at_text_start() {
    let sql_with_cursor = r#""e".| FROM "Employees" e"#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier.as_deref(), Some("e"));
}

#[test]
fn qualifier_before_word_rejects_unbalanced_quoted_identifier() {
    let sql_with_cursor = r#"SELECT "e.| FROM emp e"#;
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_rejects_unbalanced_backtick_quoted_identifier() {
    let sql_with_cursor = "SELECT `e.| FROM emp e";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn qualifier_before_word_rejects_unbalanced_bracket_quoted_identifier() {
    let sql_with_cursor = "SELECT [e.| FROM emp e";
    let cursor = sql_with_cursor.find('|').unwrap_or(0);
    let sql = sql_with_cursor.replace('|', "");
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor);
    assert_eq!(qualifier, None);
}

#[test]
fn identifier_at_position_rejects_unbalanced_quoted_identifier() {
    let sql = r#"SELECT "사용자 FROM dual"#;
    let cursor = sql.find("사용자").unwrap_or(0) + "사용자".len();

    let resolved = SqlEditorWidget::identifier_at_position_in_text(sql, cursor);
    assert!(
        resolved.is_none(),
        "unbalanced quoted identifier should not be resolved"
    );
}

#[test]
fn identifier_at_position_rejects_unbalanced_backtick_quoted_identifier() {
    let sql = "SELECT `사용자 FROM dual";
    let cursor = sql.find("사용자").unwrap_or(0) + "사용자".len();

    let resolved = SqlEditorWidget::identifier_at_position_in_text(sql, cursor);
    assert!(
        resolved.is_none(),
        "unbalanced backtick-quoted identifier should not be resolved"
    );
}

#[test]
fn identifier_at_position_rejects_unbalanced_bracket_quoted_identifier() {
    let sql = "SELECT [사용자 FROM dual";
    let cursor = sql.find("사용자").unwrap_or(0) + "사용자".len();

    let resolved = SqlEditorWidget::identifier_at_position_in_text(sql, cursor);
    assert!(
        resolved.is_none(),
        "unbalanced bracket-quoted identifier should not be resolved"
    );
}

#[test]
fn parse_dropped_file_token_decodes_utf8_percent_sequences() {
    let token = "file:///tmp/%ED%95%9C%EA%B8%80.sql";
    let parsed = SqlEditorWidget::parse_dropped_file_token(token);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/한글.sql")));
}

#[test]
fn parse_dropped_file_token_handles_case_insensitive_prefixes() {
    let token = "FiLe://LOCALHOST/tmp/My%20File.sql";
    let parsed = SqlEditorWidget::parse_dropped_file_token(token);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/My File.sql")));
}

#[test]
fn parse_dropped_file_token_strips_wrapping_quotes() {
    let token = "\"file:///tmp/Quoted%20Name.sql\"";
    let parsed = SqlEditorWidget::parse_dropped_file_token(token);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/Quoted Name.sql")));

    let single_quoted = "'file:///tmp/Single%20Quoted.sql'";
    let parsed = SqlEditorWidget::parse_dropped_file_token(single_quoted);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/Single Quoted.sql")));
}

#[test]
fn pointer_position_tracking_is_skipped_while_file_drop_is_pending() {
    let state = Arc::new(Mutex::new(DndDropState::Idle));

    assert!(!SqlEditorWidget::should_skip_pointer_position_tracking(
        &state
    ));

    SqlEditorWidget::set_dnd_drop_state(
        &state,
        DndDropState::AwaitingPaste(PendingDndDrop { position: None }),
    );

    assert!(SqlEditorWidget::should_skip_pointer_position_tracking(
        &state
    ));
}

#[test]
fn take_pending_dnd_drop_preserves_position_and_resets_state_to_idle() {
    let state = Arc::new(Mutex::new(DndDropState::AwaitingPaste(PendingDndDrop {
        position: Some(42),
    })));

    assert_eq!(
        SqlEditorWidget::take_pending_dnd_drop(&state),
        Some(PendingDndDrop { position: Some(42) })
    );
    assert_eq!(SqlEditorWidget::take_pending_dnd_drop(&state), None);
    assert!(!SqlEditorWidget::should_skip_pointer_position_tracking(
        &state
    ));
}

#[test]
fn object_drag_payload_is_not_treated_as_file_drop_path() {
    let payload = crate::ui::object_drag_payload::encode("EMPLOYEES");

    assert!(SqlEditorWidget::extract_dropped_file_path(&payload).is_none());
}

#[test]
fn typed_char_from_key_event_falls_back_for_shifted_underscore() {
    let ch = SqlEditorWidget::typed_char_from_key_event("", Key::from_char('-'), true, None);
    assert_eq!(ch, Some('_'));
}

#[test]
fn typed_char_from_key_event_infers_underscore_from_buffer_even_without_shift_state() {
    let ch = SqlEditorWidget::typed_char_from_key_event("", Key::from_char('-'), false, Some('_'));
    assert_eq!(ch, Some('_'));
}

#[test]
fn typed_char_from_key_event_keeps_minus_when_minus_was_inserted() {
    let ch = SqlEditorWidget::typed_char_from_key_event("", Key::from_char('-'), false, Some('-'));
    assert_eq!(ch, Some('-'));
}

#[test]
fn debounce_cursor_comparison_uses_raw_offsets() {
    assert!(SqlEditorWidget::is_same_raw_cursor_offset(10, 10));
    assert!(!SqlEditorWidget::is_same_raw_cursor_offset(10, 11));
}

#[test]
fn manual_trigger_invalidates_debounce_and_parse_generation() {
    let runtime = runtime_state_for_test(None, None, 17, 23);

    SqlEditorWidget::invalidate_manual_trigger_debounce_state(&runtime);

    assert_eq!(runtime.current_keyup_generation(), 18);
    assert_eq!(runtime.current_parse_generation(), 24);
}

#[test]
fn external_hide_clears_state_and_invalidates_generations() {
    let runtime = runtime_state_for_test(
        Some((3, 5)),
        Some(PendingIntellisense { cursor_pos: 7 }),
        41,
        9,
    );

    SqlEditorWidget::clear_intellisense_state_for_external_hide(&runtime);

    assert_eq!(runtime.current_keyup_generation(), 42);
    assert_eq!(runtime.current_parse_generation(), 10);
    assert!(runtime.completion_range().is_none());
    assert!(runtime.pending_intellisense().is_none());
}

#[test]
fn external_hide_ignores_only_inside_click_when_popup_visible() {
    assert!(SqlEditorWidget::should_ignore_external_hide_click(
        true, true
    ));
    assert!(!SqlEditorWidget::should_ignore_external_hide_click(
        true, false
    ));
    assert!(!SqlEditorWidget::should_ignore_external_hide_click(
        false, true
    ));
    assert!(!SqlEditorWidget::should_ignore_external_hide_click(
        false, false
    ));
}

#[test]
fn unfocus_hide_rule_hides_only_when_pointer_is_outside_visible_popup() {
    assert!(SqlEditorWidget::should_hide_popup_on_unfocus(true, false));
    assert!(!SqlEditorWidget::should_hide_popup_on_unfocus(true, true));
    assert!(!SqlEditorWidget::should_hide_popup_on_unfocus(false, false));
    assert!(!SqlEditorWidget::should_hide_popup_on_unfocus(false, true));
}

#[test]
fn nonblocking_popup_hide_waits_until_show_transition_finishes() {
    assert!(SqlEditorWidget::can_try_hide_intellisense_popup(
        IntellisensePopupTransitionState::Idle
    ));
    assert!(!SqlEditorWidget::can_try_hide_intellisense_popup(
        IntellisensePopupTransitionState::Showing
    ));
}

#[test]
fn escape_keydown_cancels_pending_even_when_popup_not_visible() {
    let runtime = runtime_state_for_test(
        Some((10, 12)),
        Some(PendingIntellisense { cursor_pos: 14 }),
        5,
        20,
    );

    let consumed = SqlEditorWidget::cancel_intellisense_on_escape_keydown(false, &runtime);

    assert!(!consumed);
    assert!(runtime.completion_range().is_none());
    assert!(runtime.pending_intellisense().is_none());
    assert_eq!(runtime.current_keyup_generation(), 6);
    assert_eq!(runtime.current_parse_generation(), 21);
}

#[test]
fn navigation_shortcut_clears_pending_even_when_popup_not_visible() {
    let runtime = runtime_state_for_test(
        Some((4, 8)),
        Some(PendingIntellisense { cursor_pos: 11 }),
        12,
        33,
    );

    SqlEditorWidget::invalidate_and_clear_pending_intellisense_state(&runtime);

    assert!(runtime.completion_range().is_none());
    assert!(runtime.pending_intellisense().is_none());
    assert_eq!(runtime.current_keyup_generation(), 13);
    assert_eq!(runtime.current_parse_generation(), 34);
}

#[test]
fn escape_keydown_consumes_when_popup_is_visible() {
    let runtime = runtime_state_for_test(
        Some((1, 3)),
        Some(PendingIntellisense { cursor_pos: 3 }),
        0,
        0,
    );

    let consumed = SqlEditorWidget::cancel_intellisense_on_escape_keydown(true, &runtime);

    assert!(consumed);
    assert!(runtime.completion_range().is_none());
    assert!(runtime.pending_intellisense().is_none());
    assert_eq!(runtime.current_keyup_generation(), 1);
    assert_eq!(runtime.current_parse_generation(), 1);
}

#[test]
fn min_intellisense_prefix_uses_character_count() {
    assert!(!SqlEditorWidget::has_min_intellisense_prefix(""));
    assert!(!SqlEditorWidget::has_min_intellisense_prefix("a"));
    assert!(SqlEditorWidget::has_min_intellisense_prefix("ab"));
    assert!(!SqlEditorWidget::has_min_intellisense_prefix("한"));
    assert!(SqlEditorWidget::has_min_intellisense_prefix("한글"));
}

#[test]
fn fast_path_delete_hides_popup_when_prefix_too_short_without_qualifier() {
    assert!(SqlEditorWidget::should_hide_fast_path_after_delete(
        "",
        None,
        Key::BackSpace
    ));
    assert!(SqlEditorWidget::should_hide_fast_path_after_delete(
        "a",
        None,
        Key::Delete
    ));
    assert!(!SqlEditorWidget::should_hide_fast_path_after_delete(
        "ab",
        None,
        Key::BackSpace
    ));
    assert!(!SqlEditorWidget::should_hide_fast_path_after_delete(
        "a",
        Some("t"),
        Key::BackSpace
    ));
    assert!(!SqlEditorWidget::should_hide_fast_path_after_delete(
        "a",
        None,
        Key::from_char('a')
    ));
}

#[test]
fn fast_path_filter_accepts_identifier_quote_prefix_chars() {
    assert!(SqlEditorWidget::is_fast_filter_key(
        Key::from_char('"'),
        Some('"')
    ));
    assert!(SqlEditorWidget::is_fast_filter_key(
        Key::from_char('`'),
        Some('`')
    ));
    assert!(SqlEditorWidget::is_fast_filter_key(
        Key::from_char('['),
        Some('[')
    ));
    assert!(SqlEditorWidget::is_cursor_within_completion_range(
        7,
        5,
        6,
        Key::from_char('"'),
        Some('"')
    ));
    assert!(SqlEditorWidget::is_cursor_within_completion_range(
        7,
        5,
        6,
        Key::from_char('`'),
        Some('`')
    ));
    assert!(SqlEditorWidget::is_cursor_within_completion_range(
        7,
        5,
        6,
        Key::from_char('['),
        Some('[')
    ));
}

#[test]
fn fast_path_prefix_preserves_quoted_identifier_body() {
    assert_eq!(
        SqlEditorWidget::completion_prefix_from_range_text(r#""Order I"#),
        r#""Order I"#
    );
    assert_eq!(
        SqlEditorWidget::completion_prefix_from_range_text("`Order I"),
        "`Order I"
    );
    assert_eq!(
        SqlEditorWidget::completion_prefix_from_range_text("[Order I"),
        "[Order I"
    );
    assert_eq!(
        SqlEditorWidget::completion_prefix_from_range_text("Order I"),
        "OrderI"
    );
}

#[test]
fn condition_comparison_suffix_ignores_bracket_identifier_dots() {
    assert_eq!(
        SqlEditorWidget::condition_comparison_completion_suffix("[Order.Detail] = 1"),
        None
    );
}

#[test]
fn condition_comparison_suffix_ignores_bracket_identifier_operators() {
    assert_eq!(
        SqlEditorWidget::condition_comparison_completion_suffix("[Odd = Name].status = 1"),
        Some("status = 1".to_string())
    );
}

#[test]
fn auto_trigger_forced_char_requires_qualifier_or_two_chars() {
    assert!(!SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("", None));
    assert!(!SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("a", None));
    assert!(!SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("한", None));
    assert!(SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("ab", None));
    assert!(SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("한글", None));
    assert!(SqlEditorWidget::should_auto_trigger_intellisense_for_forced_char("", Some("t")));
}

#[test]
fn keyup_after_manual_ctrl_space_trigger_is_ignored() {
    assert!(SqlEditorWidget::should_ignore_keyup_after_manual_trigger(
        Key::from_char(' '),
        Key::from_char(' '),
        true,
    ));
    assert!(!SqlEditorWidget::should_ignore_keyup_after_manual_trigger(
        Key::from_char(' '),
        Key::from_char(' '),
        false,
    ));
    assert!(!SqlEditorWidget::should_ignore_keyup_after_manual_trigger(
        Key::from_char('a'),
        Key::from_char('a'),
        true,
    ));
}

#[test]
fn shortcut_key_for_layout_falls_back_to_original_for_non_ascii_key() {
    assert_eq!(
        SqlEditorWidget::shortcut_key_for_layout(Key::from_char('ㄹ'), Key::from_char('f')),
        Key::from_char('f')
    );
}

#[test]
fn resolved_shortcut_key_matches_all_editor_ctrl_alpha_shortcuts() {
    for ascii in ['f', 'u', 'l', 'h', 'z', 'y'] {
        let resolved =
            SqlEditorWidget::shortcut_key_for_layout(Key::from_char('한'), Key::from_char(ascii));
        assert!(SqlEditorWidget::matches_alpha_shortcut(resolved, ascii));
    }
}

#[test]
fn resolved_shortcut_key_preserves_ctrl_space_and_ctrl_slash() {
    let space = SqlEditorWidget::shortcut_key_for_layout(Key::from_char('한'), Key::from_char(' '));
    assert_eq!(space, Key::from_char(' '));

    let slash = SqlEditorWidget::shortcut_key_for_layout(Key::from_char('한'), Key::from_char('/'));
    assert_eq!(slash, Key::from_char('/'));
}

#[test]
fn matches_alpha_shortcut_accepts_upper_and_lower_case() {
    assert!(SqlEditorWidget::matches_alpha_shortcut(
        Key::from_char('f'),
        'f'
    ));
    assert!(SqlEditorWidget::matches_alpha_shortcut(
        Key::from_char('F'),
        'f'
    ));
    assert!(!SqlEditorWidget::matches_alpha_shortcut(
        Key::from_char('g'),
        'f'
    ));
}

#[test]
fn token_spans_partition_handles_utf8_boundaries() {
    let sql = "SELECT 한글 FROM dual";
    let cursor = "SELECT 한".len();
    let spans = super::query_text::tokenize_sql_spanned(sql);
    let split_idx = spans.partition_point(|span| span.end <= cursor);
    let tokens: Vec<SqlToken> = spans[..split_idx]
        .iter()
        .map(|span| span.token.clone())
        .collect();
    assert_eq!(tokens.len(), 1);
    assert!(matches!(tokens.first(), Some(SqlToken::Word(word)) if word == "SELECT"));
}

#[test]
fn modifier_key_is_detected_for_shift_release() {
    assert!(SqlEditorWidget::is_modifier_key(Key::ShiftL));
    assert!(SqlEditorWidget::is_modifier_key(Key::ShiftR));
    assert!(!SqlEditorWidget::is_modifier_key(Key::from_char('a')));
}

#[test]
fn request_table_columns_releases_loading_when_connection_busy() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["EMP".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("EMP", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("column loader should emit a completion update even when lock is busy");
    assert_eq!(update.table, "EMP");
    assert!(update.columns.is_empty());
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_handles_quoted_schema_and_table_names() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["SCHEMA.TABLE.NAME".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns(
        "\"SCHEMA\".\"TABLE.NAME\"",
        &data,
        &sender,
        &connection,
    );

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("quoted schema/table names should normalize before relation lookup");
    assert_eq!(update.table, "SCHEMA.TABLE.NAME");
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_handles_backtick_quoted_schema_and_table_names() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["SCHEMA.TABLE.NAME".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("`SCHEMA`.`TABLE.NAME`", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("backtick-quoted schema/table names should normalize before relation lookup");
    assert_eq!(update.table, "SCHEMA.TABLE.NAME");
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_handles_bracket_quoted_schema_and_table_names() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["SCHEMA.TABLE.NAME".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("[SCHEMA].[TABLE.NAME]", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("bracket-quoted schema/table names should normalize before relation lookup");
    assert_eq!(update.table, "SCHEMA.TABLE.NAME");
    assert!(!update.cache_columns);
}

#[test]
fn column_load_schema_and_table_does_not_split_quoted_dotted_relation_name() {
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table(r#""SALES.OPS""#),
        None
    );
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table("`sales.ops`"),
        None
    );
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table("[sales.ops]"),
        None
    );
}

#[test]
fn column_load_schema_and_table_splits_quoted_schema_and_table_boundary() {
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table(r#""SALES"."ORDER.ITEMS""#),
        Some(("SALES".to_string(), "ORDER.ITEMS".to_string()))
    );
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table("`sales`.`order.items`"),
        Some(("sales".to_string(), "order.items".to_string()))
    );
    assert_eq!(
        SqlEditorWidget::column_load_schema_and_table("[sales].[order.items]"),
        Some(("sales".to_string(), "order.items".to_string()))
    );
}

#[test]
fn request_table_columns_keeps_exact_dotted_relation_name() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["A.B".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("A.B", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("known dotted relation name should still be used for column loading");
    assert_eq!(update.table, "A.B");
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_falls_back_to_unqualified_name() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["EMP".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("HR.EMP", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("schema-qualified names should fall back to relation key when needed");
    assert_eq!(update.table, "EMP");
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_uses_default_qualifier_for_unqualified_name() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_default_qualifier(Some("SCOTT".to_string()));
        guard.set_relation_members_for_qualifier("SCOTT", vec!["EMP".to_string()]);
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("EMP", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("selected default qualifier should drive unqualified column loading");
    assert_eq!(update.table, "SCOTT.EMP");
    assert!(!update.cache_columns);
}

#[test]
fn request_table_columns_keeps_selected_qualifier_for_qualified_name() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![("EMP".to_string(), Some(QualifiedMemberKind::Table))],
        );
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("SCOTT.EMP", &data, &sender, &connection);

    let update = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("schema-qualified names should keep the explicit qualifier");
    assert_eq!(update.table, "SCOTT.EMP");
    assert!(!update.cache_columns);
}

#[test]
fn column_loading_scope_detects_unqualified_pending_refresh() {
    let mut data = IntellisenseData::new();
    data.columns_loading.insert("EMP".to_string());
    let column_tables = vec!["emp".to_string()];
    let deps = HashMap::new();
    assert!(SqlEditorWidget::has_column_loading_for_scope(
        true,
        &column_tables,
        &deps,
        &data
    ));
}

#[test]
fn column_loading_scope_detects_default_qualified_pending_refresh() {
    let mut data = IntellisenseData::new();
    data.set_default_qualifier(Some("SCOTT".to_string()));
    data.set_relation_members_for_qualifier("SCOTT", vec!["EMP".to_string()]);
    data.columns_loading.insert("SCOTT.EMP".to_string());
    let column_tables = vec!["emp".to_string()];
    let deps = HashMap::new();
    assert!(SqlEditorWidget::has_column_loading_for_scope(
        true,
        &column_tables,
        &deps,
        &data
    ));
}

#[test]
fn column_loading_scope_detects_schema_qualified_pending_refresh() {
    let mut data = IntellisenseData::new();
    data.columns_loading.insert("EMP".to_string());
    let column_tables = vec!["hr.emp".to_string()];
    let deps = HashMap::new();
    assert!(SqlEditorWidget::has_column_loading_for_scope(
        true,
        &column_tables,
        &deps,
        &data
    ));
}

#[test]
fn request_table_columns_does_not_fallback_when_dot_is_inside_quoted_identifier() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["B".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("\"A.B\"", &data, &sender, &connection);

    let update = receiver.try_recv();
    assert!(
        update.is_err(),
        "quoted identifier with embedded dot should not fall back to unqualified key"
    );
}

#[test]
fn request_table_columns_does_not_treat_quoted_dotted_identifier_as_schema_member() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_relation_members_for_qualifier("A", vec!["B".to_string()]);
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("\"A.B\"", &data, &sender, &connection);

    let update = receiver.try_recv();
    assert!(
        update.is_err(),
        "quoted identifier with embedded dot should not be resolved as schema.member"
    );
}

#[test]
fn resolve_table_column_load_key_does_not_treat_quoted_dotted_identifier_as_schema_member() {
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier("A", vec!["B".to_string()]);

    let key = SqlEditorWidget::resolve_table_column_load_key(&data, r#""A.B""#);

    assert_eq!(key, None);
}

#[test]
fn resolve_table_column_load_key_keeps_exact_known_quoted_dotted_relation() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["A.B".to_string()];
    data.rebuild_indices();

    let key = SqlEditorWidget::resolve_table_column_load_key(&data, r#""A.B""#);

    assert_eq!(key.as_deref(), Some("A.B"));
}

#[test]
fn resolve_table_column_load_key_uses_quote_aware_qualified_segment_boundary() {
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier("SCHEMA", vec!["TABLE.NAME".to_string()]);

    let key = SqlEditorWidget::resolve_table_column_load_key(
        &data,
        r#""SCHEMA"."TABLE.NAME""#,
    );

    assert_eq!(key.as_deref(), Some("SCHEMA.TABLE.NAME"));
}

#[test]
fn resolve_table_column_load_key_uses_bracket_aware_qualified_segment_boundary() {
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier("SCHEMA", vec!["TABLE.NAME".to_string()]);

    let key = SqlEditorWidget::resolve_table_column_load_key(&data, "[SCHEMA].[TABLE.NAME]");

    assert_eq!(key.as_deref(), Some("SCHEMA.TABLE.NAME"));
}

#[test]
fn resolve_table_column_load_key_does_not_split_quoted_table_segment_for_qualifier() {
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier("SCHEMA.TABLE", vec!["NAME".to_string()]);

    let key = SqlEditorWidget::resolve_table_column_load_key(
        &data,
        r#""SCHEMA"."TABLE.NAME""#,
    );

    assert_eq!(key, None);
}

#[test]
fn resolve_table_column_load_key_does_not_split_bracket_table_segment_for_qualifier() {
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier("SCHEMA.TABLE", vec!["NAME".to_string()]);

    let key = SqlEditorWidget::resolve_table_column_load_key(&data, "[SCHEMA].[TABLE.NAME]");

    assert_eq!(key, None);
}

#[test]
fn request_table_columns_does_not_fallback_when_dot_is_inside_backtick_quoted_identifier() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["B".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("`A.B`", &data, &sender, &connection);

    let update = receiver.try_recv();
    assert!(
        update.is_err(),
        "backtick-quoted identifier with embedded dot should not fall back to unqualified key"
    );
}

#[test]
fn request_table_columns_does_not_fallback_for_invalid_qualified_identifier() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["EMP".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("HR.", &data, &sender, &connection);

    let update = receiver.try_recv();
    assert!(
        update.is_err(),
        "invalid qualified identifier should not fall back to unrelated relation key"
    );
}

#[test]
fn request_table_columns_ignores_unbalanced_quoted_identifier() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["EMP".to_string()];
        guard.rebuild_indices();
    }

    let (sender, receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let _conn_guard = connection.lock().ok();

    SqlEditorWidget::request_table_columns("\"HR\".\"EMP", &data, &sender, &connection);

    let update = receiver.try_recv();
    assert!(
        update.is_err(),
        "unbalanced quoted identifier should not trigger fallback column loading"
    );
}

#[test]
fn intellisense_data_clears_stale_column_loading_entries() {
    let mut data = IntellisenseData::new();
    assert!(data.mark_columns_loading("EMP"));
    std::thread::sleep(Duration::from_millis(2));

    let cleared = data.clear_stale_columns_loading(Duration::from_millis(1));
    assert_eq!(cleared, 1);
    assert!(!data.columns_loading.contains("EMP"));
}

#[test]
fn expand_virtual_table_wildcards_uses_loaded_base_table_columns() {
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["HELP".to_string()];
        guard.rebuild_indices();
        guard.set_columns_for_table("HELP", vec!["TOPIC".to_string(), "TEXT".to_string()]);
    }

    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let tokens = SqlEditorWidget::tokenize_sql("SELECT * FROM help");
    let tables_in_scope = intellisense_context::collect_tables_in_statement(&tokens);

    let (columns, tables) = SqlEditorWidget::expand_virtual_table_wildcards(
        &tokens,
        &tables_in_scope,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    let upper_tables: Vec<String> = tables.into_iter().map(|t| t.to_uppercase()).collect();
    assert_eq!(upper_tables, vec!["HELP"]);
    assert_eq!(columns, vec!["TOPIC", "TEXT"]);
}

#[test]
fn collect_context_name_suggestions_in_non_table_context_include_aliases_and_ctes() {
    let full = SqlEditorWidget::tokenize_sql(
        "WITH recent_emp AS (SELECT empno FROM emp) SELECT  FROM emp e",
    );
    let ctx = intellisense_context::analyze_cursor_context(&full, full.len());

    let suggestions =
        SqlEditorWidget::collect_context_name_suggestions("", &ctx, SqlContext::ColumnName);
    let upper: Vec<String> = suggestions.into_iter().map(|s| s.to_uppercase()).collect();

    assert!(upper.contains(&"E".to_string()));
    assert!(upper.contains(&"RECENT_EMP".to_string()));
}

#[test]
fn collect_context_name_suggestions_include_exact_alias_prefix_match() {
    let full = SqlEditorWidget::tokenize_sql(
        "WITH recent_emp AS (SELECT empno FROM emp) SELECT  FROM emp e",
    );
    let ctx = intellisense_context::analyze_cursor_context(&full, full.len());

    let suggestions =
        SqlEditorWidget::collect_context_name_suggestions("e", &ctx, SqlContext::ColumnName);

    assert_has_case_insensitive(&suggestions, "e");
}

#[test]
fn collect_context_name_suggestions_match_quoted_cte_by_unquoted_prefix() {
    let full = SqlEditorWidget::tokenize_sql(
        r#"WITH "Recent Emp" AS (SELECT empno FROM emp) SELECT  FROM "Recent Emp""#,
    );
    let ctx = intellisense_context::analyze_cursor_context(&full, full.len());

    let suggestions =
        SqlEditorWidget::collect_context_name_suggestions("Recent", &ctx, SqlContext::ColumnName);

    assert_eq!(suggestions, vec![r#""Recent Emp""#.to_string()]);
}

#[test]
fn collect_context_name_suggestions_match_backtick_cte_by_unquoted_prefix() {
    let full = SqlEditorWidget::tokenize_sql(
        "WITH `Recent Emp` AS (SELECT empno FROM emp) SELECT  FROM `Recent Emp`",
    );
    let ctx = intellisense_context::analyze_cursor_context(&full, full.len());

    let suggestions =
        SqlEditorWidget::collect_context_name_suggestions("Recent", &ctx, SqlContext::ColumnName);

    assert_eq!(suggestions, vec!["`Recent Emp`".to_string()]);
}

#[test]
fn collect_context_name_suggestions_in_table_context_keep_only_ctes() {
    let script = "WITH recent_emp AS (SELECT empno FROM emp)\nSELECT *\nFROM emp e\nCROSS APPLY (SELECT deptno FROM dept) sub\nJOIN __CODEX_CURSOR__";
    let (_statement, _cursor, deep_ctx) = analyze_full_script_marker(script);

    let suggestions =
        SqlEditorWidget::collect_context_name_suggestions("", &deep_ctx, SqlContext::TableName);
    let upper: Vec<String> = suggestions.into_iter().map(|s| s.to_uppercase()).collect();

    assert!(
        upper.contains(&"RECENT_EMP".to_string()),
        "suggestions: {:?}",
        upper
    );
    assert!(
        !upper.contains(&"E".to_string()),
        "suggestions: {:?}",
        upper
    );
    assert!(
        !upper.contains(&"SUB".to_string()),
        "suggestions: {:?}",
        upper
    );
}

#[test]
fn collect_clause_wildcard_suggestions_for_select_list_include_star_and_scoped_rowsources() {
    let deep_ctx = analyze_inline_cursor_sql(
        "WITH recent_emp AS (SELECT empno FROM emp) \
         SELECT | \
         FROM emp e \
         JOIN recent_emp ON recent_emp.empno = e.empno \
         CROSS JOIN (SELECT deptno FROM dept) sub",
    );

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let suggestions = SqlEditorWidget::collect_clause_wildcard_suggestions("", None, &deep_ctx);

    assert_eq!(suggestions.first().map(String::as_str), Some("*"));
    assert_has_case_insensitive(&suggestions, "e.*");
    assert_has_case_insensitive(&suggestions, "recent_emp.*");
    assert_has_case_insensitive(&suggestions, "sub.*");
}

#[test]
fn collect_clause_wildcard_suggestions_match_quoted_alias_by_unquoted_prefix() {
    let deep_ctx = analyze_inline_cursor_sql(r#"SELECT | FROM emp "Recent Emp""#);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let suggestions = SqlEditorWidget::collect_clause_wildcard_suggestions(
        "Recent",
        None,
        &deep_ctx,
    );

    assert_eq!(suggestions, vec![r#""Recent Emp".*"#.to_string()]);
}

#[test]
fn collect_clause_wildcard_suggestions_match_backtick_alias_by_unquoted_prefix() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT | FROM emp `Recent Emp`");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let suggestions = SqlEditorWidget::collect_clause_wildcard_suggestions(
        "Recent",
        None,
        &deep_ctx,
    );

    assert_eq!(suggestions, vec![r#""Recent Emp".*"#.to_string()]);
}

#[test]
fn collect_clause_wildcard_suggestions_match_bracket_alias_by_unquoted_prefix() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT | FROM emp [Recent Emp]");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let suggestions = SqlEditorWidget::collect_clause_wildcard_suggestions(
        "Recent",
        None,
        &deep_ctx,
    );

    assert_eq!(suggestions, vec![r#""Recent Emp".*"#.to_string()]);
}

#[test]
fn collect_clause_wildcard_suggestions_for_qualified_select_return_bare_star() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT e.| FROM emp e");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);

    let suggestions =
        SqlEditorWidget::collect_clause_wildcard_suggestions("", Some("e"), &deep_ctx);

    assert_eq!(suggestions, vec!["*".to_string()]);
}

#[test]
fn collect_clause_wildcard_suggestions_outside_select_list_are_empty() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM emp e WHERE |");

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::WhereClause);

    let suggestions = SqlEditorWidget::collect_clause_wildcard_suggestions("", None, &deep_ctx);

    assert!(suggestions.is_empty(), "suggestions: {:?}", suggestions);
}

#[test]
fn qualified_condition_comparison_suggestions_cover_supported_predicate_clauses() {
    let cases = [
        (
            "SELECT * FROM tb1 a JOIN tb2 b ON a.|",
            intellisense_context::SqlPhase::JoinCondition,
        ),
        (
            "SELECT * FROM tb1 a JOIN tb2 b ON a.id = b.id WHERE a.|",
            intellisense_context::SqlPhase::WhereClause,
        ),
        (
            "SELECT a.id FROM tb1 a JOIN tb2 b ON a.id = b.id GROUP BY a.id HAVING a.|",
            intellisense_context::SqlPhase::HavingClause,
        ),
        (
            "SELECT * FROM tb1 a START WITH a.| CONNECT BY PRIOR a.id = a.parent_id",
            intellisense_context::SqlPhase::StartWithClause,
        ),
        (
            "SELECT * FROM tb1 a CONNECT BY a.| = PRIOR a.parent_id",
            intellisense_context::SqlPhase::ConnectByClause,
        ),
        (
            "SELECT * FROM oqt_t_emp MATCH_RECOGNIZE ( PATTERN (a b+) DEFINE b AS b.| > PREV(b.sal) )",
            intellisense_context::SqlPhase::MatchRecognizeClause,
        ),
    ];

    for (sql_with_cursor, expected_phase) in cases {
        let deep_ctx = analyze_inline_cursor_sql(sql_with_cursor);
        assert_eq!(deep_ctx.phase, expected_phase, "sql: {sql_with_cursor}");
        assert!(
            SqlEditorWidget::supports_qualified_condition_comparison_suggestions(deep_ctx.phase),
            "phase should support comparison suggestions: {:?}",
            deep_ctx.phase
        );
    }
}

#[test]
fn qualified_condition_comparison_suggestions_match_same_named_columns_from_other_scopes() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string(), "only_a".to_string()]);
    data.set_columns_for_table("tb2", vec!["abc".to_string(), "only_b".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "a", &deep_ctx,
    );

    assert_has_case_insensitive(&suggestions, "a.abc = b.abc");
    assert!(
        !suggestions
            .iter()
            .any(|item| item.eq_ignore_ascii_case("a.only_a = b.only_a")),
        "unexpected unmatched comparison suggestion: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_prioritize_current_join_target() {
    let deep_ctx = analyze_inline_cursor_sql(
        "select * from help a\njoin help b on 1=1\njoin help c on b.|",
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["help".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("help", vec!["id".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "b", &deep_ctx,
    );

    let c_idx = suggestions
        .iter()
        .position(|item| item.eq_ignore_ascii_case("b.id = c.id"));
    let a_idx = suggestions
        .iter()
        .position(|item| item.eq_ignore_ascii_case("b.id = a.id"));

    assert!(
        c_idx.is_some() && a_idx.is_some() && c_idx < a_idx,
        "current join target (c) should be suggested before earlier table (a): {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_prefer_aliases_for_other_side() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.de|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["deptno".to_string(), "abc".to_string()]);
    data.set_columns_for_table("tb2", vec!["deptno".to_string(), "abc".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "de", "a", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec!["a.deptno = b.deptno".to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_use_alias_column_list_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT *
FROM oqt_t_emp e(emp_id_alias, dept_common)
JOIN oqt_t_dept d(dept_common, dept_name_alias) ON e.|
"#,
    );
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "oqt_t_emp",
        vec!["empno".to_string(), "deptno".to_string()],
    );
    data.set_columns_for_table(
        "oqt_t_dept",
        vec!["deptno".to_string(), "dname".to_string()],
    );
    data.set_virtual_table_columns(
        "e",
        vec!["emp_id_alias".to_string(), "dept_common".to_string()],
    );
    data.set_virtual_table_columns(
        "d",
        vec!["dept_common".to_string(), "dept_name_alias".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "e", &deep_ctx,
    );

    assert_has_case_insensitive(&suggestions, "e.dept_common = d.dept_common");
    assert!(
        !suggestions
            .iter()
            .any(|item| item.eq_ignore_ascii_case("e.deptno = d.deptno")),
        "alias-list comparison should not leak source columns: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_use_pivot_output_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT *
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
JOIN oqt_t_dept d ON p.|
"#,
    );
    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["deptno".to_string(), "job".to_string(), "sal".to_string()],
        );
        guard.set_columns_for_table(
            "oqt_t_dept",
            vec!["deptno".to_string(), "dname".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &lock_or_recover(&data),
        "",
        "p",
        &deep_ctx,
    );

    assert_has_case_insensitive(&suggestions, "p.deptno = d.deptno");
    assert!(
        !suggestions
            .iter()
            .any(|item| item.eq_ignore_ascii_case("p.sal = d.sal")),
        "PIVOT comparison should not leak aggregate source columns: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_are_empty_without_other_scope() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a WHERE a.|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string(), "deptno".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "a", &deep_ctx,
    );

    assert!(
        suggestions.is_empty(),
        "single-scope condition should not suggest self-comparisons: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_skip_self_when_qualifier_is_quoted_alias() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"SELECT * FROM tb1 "Dept Alias" JOIN tb2 b ON "Dept Alias".|"#,
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["deptno".to_string()]);
    data.set_columns_for_table("tb2", vec!["deptno".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data,
        "",
        r#""Dept Alias""#,
        &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec![r#""Dept Alias".deptno = b.deptno"#.to_string()],
        "quoted alias qualifier should not compare a table with itself: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_are_empty_outside_predicate_clause() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT a.| FROM tb1 a JOIN tb2 b ON a.id = b.id");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string()]);
    data.set_columns_for_table("tb2", vec!["abc".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "a", &deep_ctx,
    );

    assert!(
        suggestions.is_empty(),
        "non-predicate clause should not get comparison suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_quote_column_identifiers_when_needed() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.Or|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["Order Id".to_string(), "Only A".to_string()]);
    data.set_columns_for_table("tb2", vec!["Order Id".to_string(), "Only B".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "Or", "a", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec!["a.\"Order Id\" = b.\"Order Id\"".to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_match_quoted_display_columns_by_unquoted_prefix() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.Or|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table(
        "tb1",
        vec![r#""Order Id""#.to_string(), r#""Only A""#.to_string()],
    );
    data.set_columns_for_table(
        "tb2",
        vec![r#""Order Id""#.to_string(), r#""Only B""#.to_string()],
    );

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "Or", "a", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec![r#"a."Order Id" = b."Order Id""#.to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_preserve_backtick_display_columns() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.Or|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table(
        "tb1",
        vec!["`Order Id`".to_string(), "`Only A`".to_string()],
    );
    data.set_columns_for_table(
        "tb2",
        vec!["`Order Id`".to_string(), "`Only B`".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "Or", "a", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec!["a.`Order Id` = b.`Order Id`".to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_include_correlated_outer_aliases() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT * FROM emp e WHERE EXISTS (SELECT 1 FROM dept d WHERE e.de|)",
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["emp".to_string(), "dept".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("emp", vec!["deptno".to_string(), "empno".to_string()]);
    data.set_columns_for_table("dept", vec!["deptno".to_string(), "dname".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "de", "e", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec!["e.deptno = d.deptno".to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_use_pattern_variables_in_match_recognize() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT * FROM oqt_t_emp \
         MATCH_RECOGNIZE ( \
           PATTERN (a b+) \
           DEFINE b AS b.sa| > PREV(b.sal) \
         )",
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["oqt_t_emp".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("oqt_t_emp", vec!["sal".to_string(), "deptno".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "sa", "b", &deep_ctx,
    );

    assert_eq!(
        suggestions,
        vec!["b.sal = a.sal".to_string()],
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_suggestions_show_when_cursor_is_before_prefix_char() {
    let sql_with_cursor = "SELECT * FROM tb1 a JOIN tb2 b ON a.|a";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (prefix, word_start, _word_end) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, word_start);
    let deep_ctx = analyze_inline_cursor_sql(sql_with_cursor);

    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string(), "deptno".to_string()]);
    data.set_columns_for_table("tb2", vec!["abc".to_string(), "deptno".to_string()]);

    assert_eq!(prefix, "");
    assert_eq!(qualifier.as_deref(), Some("a"));

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data,
        &prefix,
        qualifier.as_deref().expect("expected qualifier"),
        &deep_ctx,
    );

    assert_has_case_insensitive(&suggestions, "a.abc = b.abc");
}

#[test]
fn qualified_condition_comparison_suggestions_show_for_partial_prefix_after_qualifier() {
    let sql_with_cursor = "SELECT * FROM tb1 a JOIN tb2 b ON a.a|";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (prefix, word_start, _word_end) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, word_start);
    let deep_ctx = analyze_inline_cursor_sql(sql_with_cursor);

    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string(), "deptno".to_string()]);
    data.set_columns_for_table("tb2", vec!["abc".to_string(), "deptno".to_string()]);

    assert_eq!(prefix, "a");
    assert_eq!(qualifier.as_deref(), Some("a"));

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data,
        &prefix,
        qualifier.as_deref().expect("expected qualifier"),
        &deep_ctx,
    );

    assert_has_case_insensitive(&suggestions, "a.abc = b.abc");
}

#[test]
fn qualified_condition_comparison_lookup_tables_include_join_peers_before_equals() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.a|");

    let lookup_tables = SqlEditorWidget::comparison_lookup_tables_for_context(Some("a"), &deep_ctx);

    assert!(
        lookup_tables
            .iter()
            .any(|table| table.eq_ignore_ascii_case("tb1")),
        "expected current table lookup in {:?}",
        lookup_tables
    );
    assert!(
        lookup_tables
            .iter()
            .any(|table| table.eq_ignore_ascii_case("tb2")),
        "expected peer join table lookup in {:?}",
        lookup_tables
    );
}

#[test]
fn qualified_condition_comparison_suggestions_skip_tables_declared_after_cursor_join() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT * FROM tb1 a JOIN tb2 b ON a.| JOIN tb3 c ON 1=1",
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string(), "tb3".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["id".to_string(), "abc".to_string()]);
    data.set_columns_for_table("tb2", vec!["id".to_string(), "abc".to_string()]);
    data.set_columns_for_table("tb3", vec!["id".to_string(), "abc".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "a", &deep_ctx,
    );

    assert!(
        suggestions.iter().any(|s| s.eq_ignore_ascii_case("a.id = b.id")),
        "join partner `b` should be suggested: {:?}",
        suggestions,
    );
    assert!(
        !suggestions.iter().any(|s| s.eq_ignore_ascii_case("a.id = c.id")
            || s.eq_ignore_ascii_case("a.abc = c.abc")),
        "table `c` is declared after the current JOIN ON, should not be suggested: {:?}",
        suggestions,
    );
}

#[test]
fn qualified_condition_comparison_suggestions_skip_later_join_inside_derived_subquery() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT * FROM (SELECT * FROM tb1 a JOIN tb2 b ON a.| JOIN tb3 c ON 1=1) x",
    );
    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string(), "tb3".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["id".to_string()]);
    data.set_columns_for_table("tb2", vec!["id".to_string()]);
    data.set_columns_for_table("tb3", vec!["id".to_string()]);

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data, "", "a", &deep_ctx,
    );

    assert!(
        suggestions.iter().any(|s| s.eq_ignore_ascii_case("a.id = b.id")),
        "join partner `b` should be suggested even inside a derived subquery: {:?}",
        suggestions,
    );
    assert!(
        !suggestions.iter().any(|s| s.eq_ignore_ascii_case("a.id = c.id")),
        "later-declared `c` should be filtered out inside derived subquery too: {:?}",
        suggestions,
    );
}

#[test]
fn qualified_condition_comparison_suggestions_are_suppressed_on_rhs_of_existing_equals() {
    let sql_with_cursor = "SELECT * FROM tb1 a JOIN tb2 b ON a.abc = b.ab|";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (prefix, word_start, _word_end) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, word_start);
    let deep_ctx = analyze_inline_cursor_sql(sql_with_cursor);

    let mut data = IntellisenseData::new();
    data.tables = vec!["tb1".to_string(), "tb2".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("tb1", vec!["abc".to_string(), "deptno".to_string()]);
    data.set_columns_for_table("tb2", vec!["abc".to_string(), "deptno".to_string()]);

    assert_eq!(prefix, "ab");
    assert_eq!(qualifier.as_deref(), Some("b"));

    let suggestions = SqlEditorWidget::collect_qualified_condition_comparison_suggestions(
        &data,
        &prefix,
        qualifier.as_deref().expect("expected qualifier"),
        &deep_ctx,
    );

    assert!(
        suggestions.is_empty(),
        "comparison suggestions should be suppressed on RHS after existing '=': {:?}",
        suggestions
    );
}

#[test]
fn qualified_condition_comparison_lookup_tables_are_empty_on_rhs_of_existing_equals() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM tb1 a JOIN tb2 b ON a.abc = b.ab|");

    let lookup_tables = SqlEditorWidget::comparison_lookup_tables_for_context(Some("b"), &deep_ctx);

    assert!(
        lookup_tables.is_empty(),
        "comparison lookup tables should be empty on RHS after existing '=': {:?}",
        lookup_tables
    );
}

#[test]
fn base_suggestions_for_table_context_with_prefix_stay_relation_only() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["CONFIG".to_string()];
    data.views = vec!["COUNTS_VIEW".to_string()];
    data.rebuild_indices();

    let suggestions = SqlEditorWidget::base_suggestions_for_context(
        &mut data,
        "co",
        None,
        None,
        false,
        SqlContext::TableName,
        false,
        None,
    );

    assert_has_case_insensitive(&suggestions, "CONFIG");
    assert_has_case_insensitive(&suggestions, "COUNTS_VIEW");
    assert!(
        !suggestions.iter().any(|s| s == "COLUMN"),
        "table context should not leak SQL keywords: {:?}",
        suggestions
    );
    assert!(
        !suggestions.iter().any(|s| s == "COALESCE()"),
        "table context should not leak Oracle functions: {:?}",
        suggestions
    );
    assert!(
        !suggestions.iter().any(|s| s == "COUNT()"),
        "table context should not leak aggregate functions: {:?}",
        suggestions
    );
}

#[test]
fn base_suggestions_for_restricted_column_context_with_prefix_stay_column_only() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["CONFIG".to_string()];
    data.views = vec!["COUNTS_VIEW".to_string()];
    data.rebuild_indices();
    data.set_columns_for_table("EMP", vec!["CODE".to_string(), "COUNT_TOTAL".to_string()]);
    let column_scope = vec!["EMP".to_string()];

    let suggestions = SqlEditorWidget::base_suggestions_for_context(
        &mut data,
        "co",
        None,
        Some(column_scope.as_slice()),
        true,
        SqlContext::ColumnName,
        true,
        None,
    );

    assert_has_case_insensitive(&suggestions, "CODE");
    assert_has_case_insensitive(&suggestions, "COUNT_TOTAL");
    assert!(
        !suggestions.iter().any(|s| s.eq_ignore_ascii_case("CONFIG")),
        "restricted column context should not leak relation names: {:?}",
        suggestions
    );
    assert!(
        !suggestions.iter().any(|s| s == "COLUMN"),
        "restricted column context should not leak SQL keywords: {:?}",
        suggestions
    );
    assert!(
        !suggestions.iter().any(|s| s == "COALESCE()"),
        "restricted column context should not leak Oracle functions: {:?}",
        suggestions
    );
}

#[test]
fn merge_suggestions_with_context_aliases_prioritizes_context_items_when_requested() {
    let merged = SqlEditorWidget::merge_suggestions_with_context_aliases(
        vec!["EMP".to_string(), "SELECT".to_string()],
        vec!["recent_emp".to_string(), "EMP".to_string()],
        true,
    );

    assert_eq!(merged[0], "recent_emp");
    assert!(merged.contains(&"EMP".to_string()));
    assert!(merged.contains(&"SELECT".to_string()));
}

#[test]
fn merge_suggestions_with_context_aliases_dedups_quoted_identifier_equivalents() {
    let merged = SqlEditorWidget::merge_suggestions_with_context_aliases(
        vec![r#""Recent Emp""#.to_string(), "DEPTNO".to_string()],
        vec!["Recent Emp".to_string(), "deptno".to_string()],
        true,
    );

    assert_eq!(merged, vec![r#""Recent Emp""#.to_string(), "DEPTNO".to_string()]);
}

#[test]
fn merge_suggestions_with_context_aliases_limits_to_max_suggestions() {
    let base: Vec<String> = (0..MAX_MERGED_SUGGESTIONS)
        .map(|i| format!("BASE_{:02}", i))
        .collect();
    let aliases = vec!["e".to_string(), "x".to_string()];

    let merged = SqlEditorWidget::merge_suggestions_with_context_aliases(base, aliases, true);

    assert_eq!(merged.len(), MAX_MERGED_SUGGESTIONS);
    assert_eq!(merged[0], "e");
    assert_eq!(merged[1], "x");
    assert!(!merged.contains(&format!("BASE_{:02}", MAX_MERGED_SUGGESTIONS - 1)));
}

#[test]
fn merge_suggestions_with_context_aliases_respects_max_without_aliases() {
    let base: Vec<String> = (0..(MAX_MERGED_SUGGESTIONS + 5))
        .map(|i| format!("BASE_{:02}", i))
        .collect();

    let merged = SqlEditorWidget::merge_suggestions_with_context_aliases(base, vec![], false);

    assert_eq!(merged.len(), MAX_MERGED_SUGGESTIONS);
}

#[test]
fn dedup_column_names_case_insensitive_dedups_quoted_identifier_equivalents() {
    let mut columns = vec![
        r#""Dept No""#.to_string(),
        "Dept No".to_string(),
        "`Dept No`".to_string(),
        "ENAME".to_string(),
    ];

    SqlEditorWidget::dedup_column_names_case_insensitive(&mut columns);

    assert_eq!(columns, vec![r#""Dept No""#.to_string(), "ENAME".to_string()]);
}

#[test]
fn merge_qualified_condition_comparison_suggestions_prioritizes_join_condition_matches() {
    let merged = SqlEditorWidget::merge_qualified_condition_comparison_suggestions(
        vec!["abc".to_string(), "deptno".to_string()],
        vec!["a.abc = b.abc".to_string()],
        intellisense_context::SqlPhase::JoinCondition,
    );

    assert_eq!(merged[0], "a.abc = b.abc");
    assert_eq!(merged[1], "abc");
}

#[test]
fn merge_qualified_condition_comparison_suggestions_deprioritizes_where_clause_matches() {
    let merged = SqlEditorWidget::merge_qualified_condition_comparison_suggestions(
        vec!["abc".to_string(), "deptno".to_string()],
        vec!["a.abc = b.abc".to_string()],
        intellisense_context::SqlPhase::WhereClause,
    );

    assert_eq!(merged[0], "abc");
    assert_eq!(merged[1], "deptno");
    assert_eq!(merged[2], "a.abc = b.abc");
}

#[test]
fn maybe_merge_suggestions_with_context_aliases_skips_aliases_when_qualified() {
    let base = vec!["EMPNO".to_string(), "ENAME".to_string()];
    let aliases = vec!["e".to_string(), "emp".to_string()];

    let merged = SqlEditorWidget::maybe_merge_suggestions_with_context_aliases(
        base.clone(),
        aliases,
        false,
        true,
    );

    assert_eq!(merged, base);
}

#[test]
fn local_symbol_suggestions_include_var_command_before_cursor() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        "VAR v_rc REFCURSOR;\nBEGIN\n    __CODEX_CURSOR__NULL;\nEND;",
        &[],
    );

    assert_has_case_insensitive(&suggestions, "V_RC");
}

#[test]
fn local_symbol_suggestions_include_routine_parameters_and_locals() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE OR REPLACE PROCEDURE demo_proc (
    p_empno IN NUMBER,
    p_name  IN VARCHAR2
) IS
    v_total NUMBER := 0;
    c_status CONSTANT VARCHAR2(1) := 'Y';
BEGIN
    __CODEX_CURSOR__NULL;
END demo_proc;"#,
        &[],
    );

    for expected in ["p_empno", "p_name", "v_total", "c_status"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_symbol_suggestions_preserve_quoted_local_variable_names() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    "Employee Name" VARCHAR2(100);
    "Status.Flag" NUMBER;
BEGIN
    __CODEX_CURSOR__NULL;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, r#""Employee Name""#);
    assert_has_case_insensitive(&suggestions, r#""Status.Flag""#);
    assert!(
        !suggestions
            .iter()
            .any(|name| name == "Employee Name" || name == "Status.Flag"),
        "quoted local symbols should remain insertable: {:?}",
        suggestions
    );
}

#[test]
fn local_symbol_suggestions_match_quoted_local_variable_by_unquoted_prefix() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_with_prefix_for_test(
        r#"DECLARE
    "Employee Name" VARCHAR2(100);
    normal_name VARCHAR2(100);
BEGIN
    __CODEX_CURSOR__NULL;
END;"#,
        "Emp",
        &[],
    );

    assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
}

#[test]
fn local_symbol_suggestions_match_quoted_local_variable_by_quoted_prefix() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_with_prefix_for_test(
        r#"DECLARE
    "Employee Name" VARCHAR2(100);
    normal_name VARCHAR2(100);
BEGIN
    __CODEX_CURSOR__NULL;
END;"#,
        r#""Emp"#,
        &[],
    );

    assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
}

#[test]
fn local_symbol_lookup_matches_incomplete_bracket_quoted_prefix() {
    assert_eq!(SqlEditorWidget::local_identifier_lookup_upper("[Emp"), "EMP");
    assert_eq!(
        SqlEditorWidget::local_member_suggestion_lookup_upper("[Street"),
        "STREET"
    );
}

#[test]
fn local_symbol_suggestions_preserve_quoted_routine_parameter_names() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_with_prefix_for_test(
        r#"CREATE OR REPLACE PROCEDURE demo_proc (
    "Employee Name" IN VARCHAR2,
    p_name IN VARCHAR2
) IS
BEGIN
    __CODEX_CURSOR__NULL;
END demo_proc;"#,
        r#""Emp"#,
        &[],
    );

    assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
}

#[test]
fn local_symbol_suggestions_preserve_quoted_cursor_names() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_with_prefix_for_test(
        r#"DECLARE
    CURSOR "Employee Cursor" IS
        SELECT empno FROM emp;
BEGIN
    __CODEX_CURSOR__NULL;
END;"#,
        r#""Emp"#,
        &[],
    );

    assert_eq!(suggestions, vec![r#""Employee Cursor""#.to_string()]);
}

#[test]
fn local_symbol_suggestions_keep_only_visible_nested_block_symbols() {
    let inner_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    v_outer NUMBER := 1;
BEGIN
    DECLARE
        v_outer VARCHAR2(10) := 'inner';
        v_inner NUMBER := 2;
    BEGIN
        __CODEX_CURSOR__NULL;
    END;
END;"#,
        &[],
    );
    let outer_name_count = inner_suggestions
        .iter()
        .filter(|name| name.eq_ignore_ascii_case("v_outer"))
        .count();

    assert_eq!(outer_name_count, 1);
    assert_has_case_insensitive(&inner_suggestions, "v_inner");

    let outer_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    v_outer NUMBER := 1;
BEGIN
    DECLARE
        v_inner NUMBER := 2;
    BEGIN
        NULL;
    END;

    __CODEX_CURSOR__NULL;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&outer_suggestions, "v_outer");
    assert!(
        !outer_suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("v_inner")),
        "inner block symbol should not remain visible after END: {:?}",
        outer_suggestions
    );
}

#[test]
fn local_symbol_suggestions_prefer_nearest_shadowed_symbol_display() {
    let cases = [
        (
            r#"DECLARE
    v_shadow NUMBER := 1;
BEGIN
    DECLARE
        "v_shadow" VARCHAR2(10) := 'inner';
    BEGIN
        __CODEX_CURSOR__NULL;
    END;
END;"#,
            "v_",
            r#""v_shadow""#,
        ),
        (
            r#"DECLARE
    "v_shadow" NUMBER := 1;
BEGIN
    DECLARE
        v_shadow VARCHAR2(10) := 'inner';
    BEGIN
        __CODEX_CURSOR__NULL;
    END;
END;"#,
            "v_",
            "v_shadow",
        ),
        (
            r#"CREATE OR REPLACE PROCEDURE demo_proc (
    p_shadow IN NUMBER
) IS
BEGIN
    DECLARE
        "p_shadow" VARCHAR2(10) := 'inner';
    BEGIN
        __CODEX_CURSOR__NULL;
    END;
END demo_proc;"#,
            "p_",
            r#""p_shadow""#,
        ),
    ];

    for (sql, prefix, expected) in cases {
        let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_with_prefix_for_test(
            sql,
            prefix,
            &[],
        );

        assert_eq!(suggestions, vec![expected.to_string()], "sql: {sql}");
    }
}

#[test]
fn local_symbol_suggestions_include_for_loop_record_only_inside_loop() {
    let loop_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (SELECT empno FROM emp) LOOP
        __CODEX_CURSOR__NULL;
    END LOOP;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&loop_suggestions, "rec");

    let after_loop_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (SELECT empno FROM emp) LOOP
        NULL;
    END LOOP;

    __CODEX_CURSOR__NULL;
END;"#,
        &[],
    );

    assert!(
        !after_loop_suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("rec")),
        "loop record should not remain visible after END LOOP: {:?}",
        after_loop_suggestions
    );
}

#[test]
fn local_record_member_suggestions_include_cursor_for_loop_projection_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT
            empno,
            ename AS employee_name,
            sal + NVL(comm, 0) total_comp
        FROM emp
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("loop record should be visible inside loop body");

    for expected in ["empno", "employee_name", "total_comp"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_preserve_quoted_cursor_projection_aliases() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT
            ename AS "Employee Name",
            sal AS normal_sal
        FROM emp
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("cursor FOR loop record should expose quoted projection aliases");

    assert_has_case_insensitive(&suggestions, r#""Employee Name""#);
    assert_has_case_insensitive(&suggestions, "normal_sal");
    assert!(
        !suggestions
            .iter()
            .any(|suggestion| suggestion == "Employee Name"),
        "quoted projection alias should remain insertable: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_match_quoted_cursor_projection_alias_by_unquoted_prefix() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT
            ename AS "Employee Name",
            sal AS normal_sal
        FROM emp
    ) LOOP
        rec.Emp__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "Emp",
    )
    .expect("quoted cursor projection alias should match by unquoted prefix");

    assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
}

#[test]
fn local_record_member_suggestions_match_quoted_cursor_projection_alias_by_quoted_prefix() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT
            ename AS "Employee Name",
            sal AS normal_sal
        FROM emp
    ) LOOP
        rec."Emp__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        r#""Emp"#,
    )
    .expect("quoted cursor projection alias should match by an incomplete quoted prefix");

    assert_eq!(suggestions, vec![r#""Employee Name""#.to_string()]);
}

#[test]
fn local_rowtype_member_suggestions_include_cursor_for_loop_select_star_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM emp
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("SELECT * loop record should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_cursor_for_loop_qualified_star_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT e.*
        FROM emp e
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("qualified wildcard loop record should expose source table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_merge_cursor_for_loop_mixed_wildcard_fields() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT e.*, sal + NVL(comm, 0) total_comp
        FROM emp e
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("cursor FOR loop mixed wildcard record should expose aliases and table columns");

    for expected in ["total_comp", "EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_cursor_for_loop_multiple_wildcard_sources() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT e.*, d.*
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "DEPTNO"],
    )
    .expect("multi-wildcard loop record should preserve rowtype sources");

    for expected in ["EMPNO", "ENAME", "DEPTNO"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_second_cursor_for_loop_wildcard_source() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT e.*, d.*
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("multi-wildcard loop record should preserve the second rowtype source");

    for expected in ["DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_cursor_for_loop_unqualified_star_join_sources() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("SELECT * join loop record should preserve every rowtype source");

    for expected in ["DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_filter_cursor_for_loop_projection_prefix() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT empno, ename AS employee_name, hiredate
        FROM emp
    ) LOOP
        rec.employee___CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "employee_",
    )
    .expect("loop record should be visible inside loop body");

    assert_eq!(suggestions, vec!["employee_name".to_string()]);
}

#[test]
fn local_record_member_suggestions_hide_cursor_for_loop_record_after_loop() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (SELECT empno, ename FROM emp) LOOP
        NULL;
    END LOOP;

    rec.__CODEX_CURSOR__
END;"#,
        "rec",
        "",
    );

    assert!(
        suggestions.is_none(),
        "loop record members should not remain visible after END LOOP: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_include_explicit_cursor_projection_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT
            empno,
            ename AS employee_name,
            sal + NVL(comm, 0) total_comp
        FROM emp;
BEGIN
    FOR rec IN c_emp LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("explicit cursor loop record should be visible inside loop body");

    for expected in ["empno", "employee_name", "total_comp"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_parameterized_explicit_cursor_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp(p_deptno NUMBER) IS
        SELECT
            empno,
            ename AS employee_name,
            sal + NVL(comm, 0) total_comp
        FROM emp
        WHERE deptno = p_deptno;
BEGIN
    FOR rec IN c_emp(10) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("parameterized explicit cursor loop record should expose projection fields");

    for expected in ["empno", "employee_name", "total_comp"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_explicit_cursor_select_star_loop_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT *
        FROM emp;
BEGIN
    FOR rec IN c_emp LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("explicit cursor SELECT * loop record should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_parameterized_explicit_cursor_star_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp(p_deptno NUMBER) IS
        SELECT *
        FROM emp
        WHERE deptno = p_deptno;
BEGIN
    FOR rec IN c_emp(10) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("parameterized explicit cursor SELECT * loop should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_explicit_cursor_loop_unqualified_star_join_sources() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_join IS
        SELECT *
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno;
BEGIN
    FOR rec IN c_join LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("explicit cursor loop over SELECT * join should preserve every rowtype source");

    for expected in ["DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_expand_inline_view_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM (
            SELECT empno employee_id, ename employee_name
            FROM emp
        ) src
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("inline-view wildcard loop record should expose virtual projection fields");

    for expected in ["employee_id", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_expand_pivot_wildcard_projection_without_source_leak() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM (
            SELECT deptno, job, sal
            FROM oqt_t_emp
        )
        PIVOT (
            SUM(sal) AS total_sal,
            COUNT(*) AS row_count
            FOR job IN ('CLERK' AS clerk, 'MANAGER' AS manager)
        ) p
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("PIVOT wildcard loop record should expose transformed projection fields");

    for expected in [
        "deptno",
        "clerk_total_sal",
        "clerk_row_count",
        "manager_total_sal",
        "manager_row_count",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|name| !name.eq_ignore_ascii_case(unexpected)),
            "PIVOT loop record should not expose source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn local_record_member_suggestions_expand_qualified_pivot_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT p.*
        FROM (
            SELECT deptno, job, sal
            FROM oqt_t_emp
        )
        PIVOT (
            SUM(sal) AS total_sal
            FOR job IN ('CLERK' AS clerk)
        ) p
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("qualified PIVOT wildcard loop record should expose transformed projection fields");

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|name| !name.eq_ignore_ascii_case(unexpected)),
            "qualified PIVOT wildcard should not expose source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn local_record_member_suggestions_expand_alias_column_list_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT e.*
        FROM oqt_t_emp e(emp_id_alias, "Emp Name Alias")
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("alias-list wildcard loop record should expose alias-list projection fields");

    assert_has_case_insensitive(&suggestions, "emp_id_alias");
    assert!(
        suggestions
            .iter()
            .any(|name| name == r#""Emp Name Alias""#),
        "expected quoted alias-list record member, got: {:?}",
        suggestions
    );
    assert!(
        suggestions
            .iter()
            .all(|name| !name.eq_ignore_ascii_case("empno")),
        "alias-list wildcard should not expose physical source column: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_expand_unqualified_alias_column_list_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM oqt_t_emp e(emp_id_alias, "Emp Name Alias")
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("unqualified alias-list wildcard loop record should expose alias-list projection fields");

    assert_has_case_insensitive(&suggestions, "emp_id_alias");
    assert!(
        suggestions
            .iter()
            .any(|name| name == r#""Emp Name Alias""#),
        "expected quoted alias-list record member, got: {:?}",
        suggestions
    );
    assert!(
        suggestions
            .iter()
            .all(|name| !name.eq_ignore_ascii_case("empno")),
        "unqualified alias-list wildcard should not expose physical source column: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_expand_multiple_alias_column_list_wildcards() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM oqt_t_emp e(emp_id_alias, emp_name_alias),
             oqt_t_emp f(manager_id_alias, manager_name_alias)
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("multi alias-list wildcard loop record should expose every alias-list projection");

    for expected in [
        "emp_id_alias",
        "emp_name_alias",
        "manager_id_alias",
        "manager_name_alias",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    assert!(
        suggestions
            .iter()
            .all(|name| !name.eq_ignore_ascii_case("empno")),
        "multi alias-list wildcard should not expose physical source column: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_expand_cte_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        WITH src AS (
            SELECT empno employee_id, ename employee_name
            FROM emp
        )
        SELECT *
        FROM src
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("CTE wildcard loop record should expose virtual projection fields");

    for expected in ["employee_id", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_expand_aliased_cte_qualified_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        WITH src AS (
            SELECT empno employee_id, ename employee_name
            FROM emp
        )
        SELECT s.*
        FROM src s
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("aliased CTE qualified wildcard loop record should expose virtual projection fields");

    for expected in ["employee_id", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_expand_explicit_cursor_inline_view_wildcard_rowtype() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_src IS
        SELECT *
        FROM (
            SELECT empno employee_id, ename employee_name
            FROM emp
        ) src;
    v_src c_src%ROWTYPE;
BEGIN
    v_src.__CODEX_CURSOR__
END;"#,
        "v_src",
        "",
    )
    .expect("cursor %ROWTYPE over inline-view wildcard should expose virtual projection fields");

    for expected in ["employee_id", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_merge_virtual_wildcard_and_real_rowtype_source() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM (
            SELECT empno employee_id
            FROM emp
        ) src
        JOIN dept d ON d.deptno = src.employee_id
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("virtual wildcard columns should merge with remaining real rowtype sources");

    for expected in ["employee_id", "DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_expand_inline_view_wildcard_rowtype_source() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        SELECT *
        FROM (
            SELECT e.*, sal + NVL(comm, 0) total_comp
            FROM emp e
        ) src
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("inline-view wildcard should expose explicit fields and propagated rowtype columns");

    for expected in ["total_comp", "EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_expand_cte_wildcard_rowtype_source() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        WITH src AS (
            SELECT *
            FROM emp
        )
        SELECT *
        FROM src
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("CTE wildcard should propagate the real rowtype source");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_expand_chained_cte_wildcard_projection() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        WITH base AS (
            SELECT empno employee_id, ename employee_name
            FROM emp
        ),
        filtered AS (
            SELECT *
            FROM base
        )
        SELECT *
        FROM filtered
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
    )
    .expect("chained CTE wildcard should expose base virtual projection fields");

    for expected in ["employee_id", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_expand_chained_cte_wildcard_rowtype_source() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"BEGIN
    FOR rec IN (
        WITH base AS (
            SELECT e.*, sal + NVL(comm, 0) total_comp
            FROM emp e
        ),
        filtered AS (
            SELECT *
            FROM base
        )
        SELECT *
        FROM filtered
    ) LOOP
        rec.__CODEX_CURSOR__
    END LOOP;
END;"#,
        "rec",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("chained CTE wildcard should expose virtual fields and propagated table columns");

    for expected in ["total_comp", "EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_explicit_cursor_qualified_star_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT e.*
        FROM emp e;
    v_emp c_emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("cursor %ROWTYPE over qualified wildcard should expose loaded source table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_merge_explicit_cursor_mixed_wildcard_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT e.*, sal + NVL(comm, 0) total_comp
        FROM emp e;
    v_emp c_emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("cursor %ROWTYPE over mixed wildcard should expose aliases and table columns");

    for expected in ["total_comp", "EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_explicit_cursor_multiple_wildcard_sources() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_join IS
        SELECT e.*, d.*
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno;
    v_join c_join%ROWTYPE;
BEGIN
    v_join.__CODEX_CURSOR__
END;"#,
        "v_join",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("cursor %ROWTYPE over multi-wildcard projection should preserve all rowtype sources");

    for expected in ["DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_explicit_cursor_unqualified_star_join_sources() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_join IS
        SELECT *
        FROM emp e
        JOIN dept d ON d.deptno = e.deptno;
    v_join c_join%ROWTYPE;
BEGIN
    v_join.__CODEX_CURSOR__
END;"#,
        "v_join",
        "",
        "DEPT",
        &["DEPTNO", "DNAME", "LOC"],
    )
    .expect("cursor %ROWTYPE over SELECT * join should preserve every rowtype source");

    for expected in ["DEPTNO", "DNAME", "LOC"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_explicit_cursor_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT
            empno,
            ename AS employee_name,
            sal + NVL(comm, 0) total_comp
        FROM emp;
    v_emp c_emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
    )
    .expect("cursor %ROWTYPE variable should expose cursor projection fields");

    for expected in ["empno", "employee_name", "total_comp"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_do_not_query_explicit_cursor_projection_name() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT empno, ename AS employee_name
        FROM emp;
    v_emp c_emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
        "C_EMP",
        &["SHOULD_NOT_APPEAR"],
    );

    assert!(
        suggestions.is_none(),
        "cursor projection %ROWTYPE should not use cursor name as a table source: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_do_not_treat_cursor_name_type_as_rowtype() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT empno, ename
        FROM emp;
    v_emp c_emp;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
    );

    assert!(
        suggestions.is_none(),
        "cursor name without %ROWTYPE should not expose cursor projection fields: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_include_declared_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno emp.empno%TYPE,
        employee_name emp.ename%TYPE,
        total_comp NUMBER
    );
    v_emp emp_rec;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
    )
    .expect("record variable should expose fields from its local record type");

    for expected in ["empno", "employee_name", "total_comp"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_nested_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        addr addr_rec
    );
    v_emp emp_rec;
BEGIN
    v_emp.addr.__CODEX_CURSOR__
END;"#,
        "v_emp.addr",
        "",
    )
    .expect("nested record field should expose fields from its local record type");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_quoted_nested_record_field_with_dot() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        "Street.Name" VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        "Addr.Info" addr_rec
    );
    v_emp emp_rec;
BEGIN
    v_emp."Addr.Info".__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("quoted nested record field with dot should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    )
    .expect("quoted nested record field with dot should expose nested record fields");

    for expected in [r#""Street.Name""#, "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_match_quoted_record_field_by_unquoted_prefix() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        "Street.Name" VARCHAR2(100),
        city VARCHAR2(100)
    );
    v_addr addr_rec;
BEGIN
    v_addr.__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("record qualifier should be available while typing a quoted field prefix");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "St",
    )
    .expect("quoted record field should be matched by its unquoted prefix");

    assert_has_case_insensitive(&suggestions, r#""Street.Name""#);
    assert!(
        !suggestions
            .iter()
            .any(|suggestion| suggestion.eq_ignore_ascii_case("city")),
        "unexpected `city` in values: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_match_quoted_record_field_by_quoted_prefix() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        "Street.Name" VARCHAR2(100),
        city VARCHAR2(100)
    );
    v_addr addr_rec;
BEGIN
    v_addr.__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("record qualifier should be available while typing a quoted field prefix");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        r#""St"#,
    )
    .expect("quoted record field should be matched by an incomplete quoted prefix");

    assert_eq!(suggestions, vec![r#""Street.Name""#.to_string()]);
}

#[test]
fn local_record_member_raw_qualifier_segments_keep_bracket_dots() {
    assert_eq!(
        SqlEditorWidget::split_raw_qualifier_segments("[Schema.Name].[Address.Record]"),
        vec!["[Schema.Name]", "[Address.Record]"]
    );
}

#[test]
fn local_record_member_suggestions_match_inline_incomplete_quoted_prefix() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        "Street.Name" VARCHAR2(100),
        city VARCHAR2(100)
    );
    v_addr addr_rec;
BEGIN
    v_addr."St__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let (prefix, word_start, word_end) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, word_start)
        .expect("record qualifier should survive an incomplete quoted field prefix");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        &prefix,
    )
    .expect("quoted record field should be matched from the inline editor prefix");
    let replacement = SqlEditorWidget::completion_replacement_range_from_word_bounds(
        &prefix,
        word_start,
        word_end,
        cursor,
        Some((word_start, cursor)),
    );

    assert_eq!(prefix, r#""St"#);
    assert_eq!(qualifier, "v_addr");
    assert_eq!(suggestions, vec![r#""Street.Name""#.to_string()]);
    assert_eq!(sql.get(replacement.0..replacement.1), Some(r#""St"#));
}

#[test]
fn local_record_member_suggestions_include_quoted_record_type_with_dot() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE "Addr.Info" IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        addr "Addr.Info"
    );
    v_emp emp_rec;
BEGIN
    v_emp.addr.__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("record field with quoted dotted type should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    )
    .expect("record field with quoted dotted type should expose nested record fields");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_nested_record_rowtype_field_columns() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    TYPE wrapper_rec IS RECORD (
        emp emp%ROWTYPE
    );
    v_wrapper wrapper_rec;
BEGIN
    v_wrapper.emp.__CODEX_CURSOR__
END;"#,
        "v_wrapper.emp",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("nested record %ROWTYPE field should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_indexed_collection_nested_record_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        addr addr_rec
    );
    TYPE emp_tab IS TABLE OF emp_rec INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps(1).addr.__CODEX_CURSOR__
END;"#,
        "v_emps.addr",
        "",
    )
    .expect("indexed collection nested record field should expose element field members");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_nested_collection_field_element_fields() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE addr_tab IS TABLE OF addr_rec INDEX BY PLS_INTEGER;
    TYPE emp_rec IS RECORD (
        addrs addr_tab
    );
    v_emp emp_rec;
BEGIN
    v_emp.addrs(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed nested collection field should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    )
    .expect("indexed nested collection field should expose element record fields");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_do_not_treat_nested_record_field_as_indexed_collection() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        addr addr_rec
    );
    v_emp emp_rec;
BEGIN
    v_emp.addr(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed nested record field should still parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    );

    assert!(
        suggestions.is_none(),
        "plain nested record field should not expose fields through indexed access: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_include_string_key_nested_collection_field_fields() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE addr_tab IS TABLE OF addr_rec INDEX BY VARCHAR2(100);
    TYPE emp_rec IS RECORD (
        addrs addr_tab
    );
    v_emp emp_rec;
BEGIN
    v_emp.addrs('HOME.WORK').__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("dotted string-key nested collection field should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    )
    .expect("dotted string-key nested collection field should expose element record fields");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_quoted_string_key_collection_field_with_dot() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100),
        city VARCHAR2(100)
    );
    TYPE addr_tab IS TABLE OF addr_rec INDEX BY VARCHAR2(100);
    TYPE emp_rec IS RECORD (
        "Addrs.Info" addr_tab
    );
    v_emp emp_rec;
BEGIN
    v_emp."Addrs.Info"('HOME.WORK').__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("quoted dotted string-key collection field should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    )
    .expect("quoted dotted string-key collection field should expose element record fields");

    for expected in ["street", "city"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_do_not_treat_dotted_string_key_plain_record_as_collection() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        addr addr_rec
    );
    v_emp emp_rec;
BEGIN
    v_emp('HOME.WORK').addr.__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("dotted string-key invalid record index should still parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    );

    assert!(
        suggestions.is_none(),
        "plain record variable should not expose fields through dotted string-key indexed access: {:?}",
        suggestions
    );
}

#[test]
fn local_rowtype_member_suggestions_include_nested_collection_field_rowtype_columns() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE emp_tab IS TABLE OF emp%ROWTYPE INDEX BY PLS_INTEGER;
    TYPE wrapper_rec IS RECORD (
        emps emp_tab
    );
    v_wrapper wrapper_rec;
BEGIN
    v_wrapper.emps(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed nested rowtype collection field should parse its qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("indexed nested rowtype collection field should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_do_not_treat_plain_record_nested_field_as_indexed_collection() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE addr_rec IS RECORD (
        street VARCHAR2(100)
    );
    TYPE emp_rec IS RECORD (
        addr addr_rec
    );
    v_emp emp_rec;
BEGIN
    v_emp(1).addr.__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed nested expression should still parse its base qualifier chain");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    );

    assert!(
        suggestions.is_none(),
        "plain record variable should not expose nested fields through indexed access: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_include_collection_element_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );
    TYPE emp_tab IS TABLE OF emp_rec INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps.__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
    )
    .expect("collection variable should expose element record fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_varray_element_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );
    TYPE emp_arr IS VARRAY(10) OF emp_rec;
    v_emps emp_arr;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
    )
    .expect("VARRAY element should expose local record fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_varying_array_element_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );
    TYPE emp_arr IS VARYING ARRAY(10) OF emp_rec;
    v_emps emp_arr;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
    )
    .expect("VARYING ARRAY element should expose local record fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_collection_element_cursor_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT empno, ename AS employee_name
        FROM emp;
    TYPE emp_tab IS TABLE OF c_emp%ROWTYPE INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
    )
    .expect("collection over cursor %ROWTYPE should expose cursor projection fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_member_suggestions_merge_collection_element_mixed_cursor_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_member_suggestions_for_test(
        r#"DECLARE
    CURSOR c_emp IS
        SELECT e.*, sal + NVL(comm, 0) total_comp
        FROM emp e;
    TYPE emp_tab IS TABLE OF c_emp%ROWTYPE INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("collection over mixed cursor %ROWTYPE should expose aliases and table columns");

    for expected in ["total_comp", "EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_do_not_treat_plain_record_as_indexed_collection() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );
    v_emp emp_rec;
BEGIN
    v_emp(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed expression should still parse its base qualifier");
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
    );

    assert!(
        suggestions.is_none(),
        "plain record variable should not expose fields through indexed access: {:?}",
        suggestions
    );
}

#[test]
fn local_rowtype_member_suggestions_include_collection_element_table_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_tab IS TABLE OF emp%ROWTYPE INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps.__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("collection variable should expose table %ROWTYPE element fields");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_varray_element_table_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_arr IS VARRAY(10) OF emp%ROWTYPE;
    v_emps emp_arr;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#,
        "v_emps",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("VARRAY element over table %ROWTYPE should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_do_not_treat_plain_rowtype_as_indexed_collection() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    v_emp emp%ROWTYPE;
BEGIN
    v_emp(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed expression should still parse its base qualifier");
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    );

    assert!(
        suggestions.is_none(),
        "plain %ROWTYPE variable should not expose fields through indexed access: {:?}",
        suggestions
    );
}

#[test]
fn local_rowtype_member_suggestions_support_indexed_collection_expression() {
    const CURSOR_MARKER: &str = "__CODEX_CURSOR__";
    let script_with_cursor = r#"DECLARE
    TYPE emp_tab IS TABLE OF emp%ROWTYPE INDEX BY PLS_INTEGER;
    v_emps emp_tab;
BEGIN
    v_emps(1).__CODEX_CURSOR__
END;"#;
    let cursor = script_with_cursor.find(CURSOR_MARKER).unwrap();
    let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
    let qualifier = SqlEditorWidget::qualifier_before_word_in_text(&sql, cursor)
        .expect("indexed collection expression should resolve to base qualifier");

    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        script_with_cursor,
        &qualifier,
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("indexed collection element should expose table %ROWTYPE fields");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_routine_parameter_collection_record_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY pkg AS
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );
    TYPE emp_tab IS TABLE OF emp_rec INDEX BY PLS_INTEGER;

    PROCEDURE sync_emp(p_emps IN emp_tab) IS
    BEGIN
        p_emps(1).__CODEX_CURSOR__
    END;
END;"#,
        "p_emps",
        "",
    )
    .expect("collection-typed routine parameter should expose element record fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_routine_parameter_collection_table_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY pkg AS
    TYPE emp_tab IS TABLE OF emp%ROWTYPE INDEX BY PLS_INTEGER;

    PROCEDURE sync_emp(p_emps IN OUT NOCOPY emp_tab) IS
    BEGIN
        p_emps(1).__CODEX_CURSOR__
    END;
END;"#,
        "p_emps",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("collection-typed routine parameter should expose table %ROWTYPE element fields");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_record_member_suggestions_include_routine_parameter_record_type_fields() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY pkg AS
    TYPE emp_rec IS RECORD (
        empno NUMBER,
        employee_name VARCHAR2(100)
    );

    PROCEDURE sync_emp(p_emp IN emp_rec) IS
    BEGIN
        p_emp.__CODEX_CURSOR__
    END;
END;"#,
        "p_emp",
        "",
    )
    .expect("record-typed routine parameter should expose local record type fields");

    for expected in ["empno", "employee_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_include_routine_parameter_table_rowtype_fields() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"CREATE OR REPLACE PROCEDURE sync_emp(p_emp IN emp%ROWTYPE) IS
BEGIN
    p_emp.__CODEX_CURSOR__
END;"#,
        "p_emp",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("table %ROWTYPE routine parameter should expose loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_ignore_routine_parameter_scalar_percent_type() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"CREATE OR REPLACE PROCEDURE sync_emp(p_name IN emp.ename%TYPE) IS
BEGIN
    p_name.__CODEX_CURSOR__
END;"#,
        "p_name",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    );

    assert!(
        suggestions.is_none(),
        "scalar %TYPE routine parameter should not expose table row fields: {:?}",
        suggestions
    );
}

#[test]
fn local_record_member_suggestions_respect_inner_scalar_shadowing() {
    let suggestions = SqlEditorWidget::collect_local_record_member_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER
    );
    v_emp emp_rec;
BEGIN
    DECLARE
        v_emp NUMBER;
    BEGIN
        v_emp.__CODEX_CURSOR__
    END;
END;"#,
        "v_emp",
        "",
    );

    assert!(
        suggestions.is_none(),
        "inner scalar variable should shadow outer record members: {:?}",
        suggestions
    );
}

#[test]
fn local_rowtype_member_suggestions_use_loaded_table_columns() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    v_emp scott.emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
        "SCOTT.EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("table %ROWTYPE variable should resolve to loaded table columns");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_use_default_qualified_table_columns() {
    let suggestions =
        SqlEditorWidget::collect_local_rowtype_member_suggestions_with_default_for_test(
            r#"DECLARE
    v_emp emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
            "v_emp",
            "",
            "SCOTT",
            "SCOTT.EMP",
            &["EMPNO", "ENAME", "SAL"],
        )
        .expect("unqualified table %ROWTYPE should use the default schema cache key");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_fall_back_to_unqualified_table_columns() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    v_emp scott.emp%ROWTYPE;
BEGIN
    v_emp.__CODEX_CURSOR__
END;"#,
        "v_emp",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    )
    .expect("qualified table %ROWTYPE should reuse an unqualified loaded cache key");

    for expected in ["EMPNO", "ENAME", "SAL"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_rowtype_member_suggestions_ignore_scalar_percent_type() {
    let suggestions = SqlEditorWidget::collect_local_rowtype_member_suggestions_for_test(
        r#"DECLARE
    v_ename emp.ename%TYPE;
BEGIN
    v_ename.__CODEX_CURSOR__
END;"#,
        "v_ename",
        "",
        "EMP",
        &["EMPNO", "ENAME", "SAL"],
    );

    assert!(
        suggestions.is_none(),
        "scalar %TYPE variable should not expose table row fields: {:?}",
        suggestions
    );
}

#[test]
fn local_symbol_suggestions_do_not_expose_record_type_metadata() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    TYPE emp_rec IS RECORD (
        empno NUMBER
    );
    v_emp emp_rec;
BEGIN
    __CODEX_CURSOR__NULL;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "v_emp");
    assert!(
        !suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("emp_rec")),
        "record type metadata should not be exposed as a local variable suggestion: {:?}",
        suggestions
    );
}

#[test]
fn local_symbol_suggestions_include_declared_exceptions() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    e_missing_data EXCEPTION;
BEGIN
    RAISE __CODEX_CURSOR__;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "e_missing_data");
}

#[test]
fn local_symbol_suggestions_rank_inner_scope_before_outer_scope() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    v_outer NUMBER := 1;
BEGIN
    DECLARE
        v_inner NUMBER := 2;
    BEGIN
        __CODEX_CURSOR__NULL;
    END;
END;"#,
        &[],
    );

    let inner_idx = suggestions
        .iter()
        .position(|name| name.eq_ignore_ascii_case("v_inner"));
    let outer_idx = suggestions
        .iter()
        .position(|name| name.eq_ignore_ascii_case("v_outer"));

    assert!(
        inner_idx.is_some(),
        "inner scope symbol should be suggested"
    );
    assert!(
        outer_idx.is_some(),
        "outer scope symbol should be suggested"
    );
    assert!(
        inner_idx < outer_idx,
        "inner scope symbol should rank before outer scope symbol: {:?}",
        suggestions
    );
}

#[test]
fn local_symbol_suggestions_keep_exception_visibility_scoped_to_nested_block() {
    let inner_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    e_outer EXCEPTION;
BEGIN
    DECLARE
        e_inner EXCEPTION;
    BEGIN
        RAISE __CODEX_CURSOR__;
    END;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&inner_suggestions, "e_outer");
    assert_has_case_insensitive(&inner_suggestions, "e_inner");

    let outer_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    e_outer EXCEPTION;
BEGIN
    DECLARE
        e_inner EXCEPTION;
    BEGIN
        NULL;
    END;

    RAISE __CODEX_CURSOR__;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&outer_suggestions, "e_outer");
    assert!(
        !outer_suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("e_inner")),
        "inner exception should not remain visible after END: {:?}",
        outer_suggestions
    );
}

#[test]
fn local_symbol_suggestions_include_package_body_outer_declarations() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
    g_cache NUMBER := 0;

    PROCEDURE run_demo IS
        v_local NUMBER := 1;
    BEGIN
        __CODEX_CURSOR__NULL;
    END run_demo;
END demo_pkg;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "g_cache");
    assert_has_case_insensitive(&suggestions, "v_local");
}

#[test]
fn local_symbol_suggestions_include_package_body_routine_in_out_parameters() {
    let procedure_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
    PROCEDURE upsert_emp(
        p_empno   IN NUMBER,
        p_ename   IN OUT VARCHAR2,
        p_message OUT VARCHAR2
    ) IS
    BEGIN
        __CODEX_CURSOR__NULL;
    END upsert_emp;
END demo_pkg;"#,
        &[],
    );

    for expected in ["p_empno", "p_ename", "p_message"] {
        assert_has_case_insensitive(&procedure_suggestions, expected);
    }

    let function_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
    FUNCTION calc_bonus(
        p_base    IN NUMBER,
        p_percent IN OUT NUMBER,
        p_error   OUT VARCHAR2
    ) RETURN NUMBER IS
    BEGIN
        __CODEX_CURSOR__NULL;
        RETURN p_base;
    END calc_bonus;
END demo_pkg;"#,
        &[],
    );

    for expected in ["p_base", "p_percent", "p_error"] {
        assert_has_case_insensitive(&function_suggestions, expected);
    }
}

#[test]
fn local_symbol_suggestions_include_package_body_parameters_when_comment_separates_name_and_paren()
{
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE OR REPLACE PACKAGE BODY demo_pkg AS
    PROCEDURE run_demo
    -- keep implementation note
    (
        p_input  IN NUMBER,
        p_output OUT VARCHAR2
    ) IS
    BEGIN
        __CODEX_CURSOR__NULL;
    END run_demo;
END demo_pkg;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "p_input");
    assert_has_case_insensitive(&suggestions, "p_output");
}

#[test]
fn local_symbol_suggestions_include_mysql_procedure_in_out_parameters() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE PROCEDURE upsert_emp(
    IN p_empno INT,
    INOUT p_ename VARCHAR(100),
    OUT p_message VARCHAR(255)
)
BEGIN
    __CODEX_CURSOR__SELECT 1;
END;"#,
        &[],
    );

    for expected in ["p_empno", "p_ename", "p_message"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn local_symbol_suggestions_include_mysql_function_parameters_for_return_body() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE FUNCTION fn_total(p_amount DECIMAL(10,2), `p_rate` DECIMAL(5,2))
RETURNS DECIMAL(10,2)
RETURN __CODEX_CURSOR__p_amount + `p_rate`;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "p_amount");
    assert_has_case_insensitive(&suggestions, "`p_rate`");
}

#[test]
fn local_symbol_suggestions_include_mysql_declared_locals_and_cursor_without_handler_noise() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE PROCEDURE demo_proc()
BEGIN
    DECLARE v_count INT DEFAULT 0;
    DECLARE `v_total` DECIMAL(10,2) DEFAULT 0;
    DECLARE cur_emp CURSOR FOR SELECT empno FROM emp;
    DECLARE CONTINUE HANDLER FOR NOT FOUND SET v_count = 1;

    __CODEX_CURSOR__SELECT v_count, `v_total` FROM dual;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "v_count");
    assert_has_case_insensitive(&suggestions, "`v_total`");
    assert_has_case_insensitive(&suggestions, "cur_emp");
    assert!(
        !suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("continue")
                || name.eq_ignore_ascii_case("handler")),
        "handler keywords must not leak into local suggestions: {:?}",
        suggestions
    );
}

#[test]
fn local_symbol_suggestions_keep_mysql_nested_block_locals_scoped() {
    let inner_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE PROCEDURE demo_proc()
BEGIN
    DECLARE v_outer INT DEFAULT 0;

    nested_block: BEGIN
        DECLARE v_inner INT DEFAULT 1;
        __CODEX_CURSOR__SELECT v_inner;
    END;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&inner_suggestions, "v_outer");
    assert_has_case_insensitive(&inner_suggestions, "v_inner");

    let outer_suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"CREATE PROCEDURE demo_proc()
BEGIN
    DECLARE v_outer INT DEFAULT 0;

    nested_block: BEGIN
        DECLARE v_inner INT DEFAULT 1;
        SELECT v_inner;
    END;

    __CODEX_CURSOR__SELECT v_outer;
END;"#,
        &[],
    );

    assert_has_case_insensitive(&outer_suggestions, "v_outer");
    assert!(
        !outer_suggestions
            .iter()
            .any(|name| name.eq_ignore_ascii_case("v_inner")),
        "nested MySQL block variable should not remain visible after END: {:?}",
        outer_suggestions
    );
}

#[test]
fn local_symbol_suggestions_include_mariadb_begin_not_atomic_declares() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"BEGIN NOT ATOMIC
    DECLARE v_count INT DEFAULT 0;
    __CODEX_CURSOR__SET v_count = v_count + 1;
END"#,
        &[],
    );

    assert_has_case_insensitive(&suggestions, "v_count");
}

#[test]
fn local_symbol_suggestions_support_select_into_and_returning_into_targets() {
    let select_into = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    v_empno NUMBER;
BEGIN
    SELECT empno INTO __CODEX_CURSOR__ FROM emp WHERE rownum = 1;
END;"#,
        &[],
    );
    assert_has_case_insensitive(&select_into, "v_empno");

    let returning_into = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        r#"DECLARE
    v_empno NUMBER;
BEGIN
    DELETE FROM emp WHERE empno = 1 RETURNING empno INTO __CODEX_CURSOR__;
END;"#,
        &[],
    );
    assert_has_case_insensitive(&returning_into, "v_empno");
}

#[test]
fn local_symbol_suggestions_merge_session_binds_without_duplicates() {
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        "VAR v_text NUMBER;\nBEGIN\n    __CODEX_CURSOR__NULL;\nEND;",
        &["V_TEXT", "V_SESSION"],
    );
    let v_text_count = suggestions
        .iter()
        .filter(|name| name.eq_ignore_ascii_case("V_TEXT"))
        .count();

    assert_eq!(v_text_count, 1);
    assert_has_case_insensitive(&suggestions, "V_SESSION");
}

#[test]
fn prepend_local_symbol_suggestions_dedups_quoted_identifier_equivalents() {
    let merged = SqlEditorWidget::prepend_local_symbol_suggestions(
        vec!["v total".to_string(), "ENAME".to_string()],
        vec![r#""v total""#.to_string(), "ename".to_string()],
    );

    assert_eq!(merged, vec![r#""v total""#.to_string(), "ename".to_string()]);
}

#[test]
fn large_routine_cache_analysis_keeps_far_declarations_visible() {
    let mut sql = String::from("CREATE OR REPLACE PROCEDURE demo_proc IS\n");
    sql.push_str("    v_far NUMBER := 1;\n");
    for idx in 0..10_000 {
        sql.push_str(&format!("    v_pad_{idx} NUMBER := {idx};\n"));
    }
    sql.push_str("BEGIN\n");
    sql.push_str("    __CODEX_CURSOR__NULL;\n");
    sql.push_str("END demo_proc;");

    let cursor = sql
        .find("__CODEX_CURSOR__")
        .expect("cursor marker should exist");
    let sql = sql.replacen("__CODEX_CURSOR__", "", 1);
    let (routine_cache, expanded) =
        SqlEditorWidget::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
    let analysis = SqlEditorWidget::build_intellisense_analysis_from_routine_cache(
        &routine_cache,
        expanded.cursor_in_statement,
    );
    let suggestions = SqlEditorWidget::collect_local_symbol_suggestions(
        "",
        expanded.cursor_in_statement,
        &analysis,
        &[],
    );

    assert!(
        sql.len() > INTELLISENSE_STATEMENT_WINDOW as usize,
        "generated procedure should exceed the default statement window"
    );
    assert_has_case_insensitive(&suggestions, "v_far");
}

#[test]
fn routine_symbol_cache_reanalyzes_cursor_context_inside_cached_statement() {
    let mut sql = String::from("SELECT e.");
    let select_cursor = sql.len();
    sql.push_str("ename FROM emp e WHERE ");
    let where_cursor = sql.len();
    sql.push_str("e.deptno = 10;\nSELECT * FROM dept");
    let other_statement_cursor = sql.rfind("dept").expect("second statement should exist");

    let (routine_cache, _expanded) =
        SqlEditorWidget::build_routine_symbol_cache_bundle_for_test(&sql, select_cursor);
    let runtime = IntellisenseRuntimeState::new();
    runtime.set_routine_symbol_cache(routine_cache);

    let select_cache = runtime
        .routine_symbol_cache_covering_cursor(0, select_cursor)
        .expect("same-statement select-list cursor should reuse routine cache");
    let select_analysis = SqlEditorWidget::build_intellisense_analysis_from_routine_cache(
        &select_cache,
        select_cursor.saturating_sub(select_cache.statement_start),
    );
    assert_eq!(
        select_analysis.context.phase,
        intellisense_context::SqlPhase::SelectList
    );

    let where_cache = runtime
        .routine_symbol_cache_covering_cursor(0, where_cursor)
        .expect("same-statement where cursor should reuse routine cache");
    let where_analysis = SqlEditorWidget::build_intellisense_analysis_from_routine_cache(
        &where_cache,
        where_cursor.saturating_sub(where_cache.statement_start),
    );
    assert_eq!(
        where_analysis.context.phase,
        intellisense_context::SqlPhase::WhereClause
    );

    assert!(
        runtime
            .routine_symbol_cache_covering_cursor(0, other_statement_cursor)
            .is_none(),
        "routine cache must not be reused across statement boundaries"
    );
    assert!(
        runtime
            .routine_symbol_cache_covering_cursor(1, where_cursor)
            .is_none(),
        "routine cache must not be reused across buffer revisions"
    );
}

#[test]
fn xmltable_alias_qualified_column_suggestions_include_columns_clause_names() {
    let sql_with_cursor = r#"
SELECT
  x.|,
  x.name
FROM oqt_t_xml t,
     XMLTABLE(
       '/root/dept'
       PASSING t.payload
       COLUMNS
         deptno NUMBER       PATH '@deptno',
         "Dept No" NUMBER    PATH '@deptno_text',
         name   VARCHAR2(30) PATH 'name/text()',
         loc    VARCHAR2(30) PATH 'loc/text()'
     ) x
ORDER BY x.deptno
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let column_tables =
        intellisense_context::resolve_qualifier_tables("x", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["x".to_string()]);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();

    for subq in &deep_ctx.subqueries {
        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            subq.body_range,
        );
        let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
        if columns.is_empty() {
            columns = intellisense_context::extract_table_function_columns(body_tokens);
        }
        let body_tables_in_scope = intellisense_context::collect_tables_in_statement(body_tokens);
        let (wildcard_columns, _wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
            body_tokens,
            &body_tables_in_scope,
            &HashMap::new(),
            &data,
            &sender,
            &connection,
        );
        columns.extend(wildcard_columns);
        SqlEditorWidget::dedup_column_names_case_insensitive(&mut columns);
        if !columns.is_empty() {
            lock_or_recover(&data).set_virtual_table_columns(&subq.alias, columns);
        }
    }

    let mut guard = lock_or_recover(&data);
    let suggestions = guard.get_column_suggestions("", Some(&column_tables));
    assert!(
        suggestions.iter().any(|c| c.eq_ignore_ascii_case("deptno")),
        "expected deptno suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|c| c == r#""Dept No""#),
        "expected quoted Dept No suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|c| c.eq_ignore_ascii_case("name")),
        "expected name suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|c| c.eq_ignore_ascii_case("loc")),
        "expected loc suggestion, got: {:?}",
        suggestions
    );
}

#[test]
fn openjson_alias_qualified_column_suggestions_include_with_clause_names() {
    let sql_with_cursor = r#"
SELECT
  oj.|
FROM orders o
CROSS APPLY OPENJSON(
  o.payload,
  '$.items'
) WITH (
  item_id int '$.id',
  "Item Id" int '$.itemId',
  item_nm nvarchar(100) '$.name',
  item_qty int '$.qty'
) oj
ORDER BY oj.item_id
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let column_tables =
        intellisense_context::resolve_qualifier_tables("oj", &deep_ctx.tables_in_scope);
    assert_eq!(column_tables, vec!["oj".to_string()]);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();

    for subq in &deep_ctx.subqueries {
        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            subq.body_range,
        );
        let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
        if columns.is_empty() {
            columns = intellisense_context::extract_table_function_columns(body_tokens);
        }
        let body_tables_in_scope = intellisense_context::collect_tables_in_statement(body_tokens);
        let (wildcard_columns, _wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
            body_tokens,
            &body_tables_in_scope,
            &HashMap::new(),
            &data,
            &sender,
            &connection,
        );
        columns.extend(wildcard_columns);
        SqlEditorWidget::dedup_column_names_case_insensitive(&mut columns);
        if !columns.is_empty() {
            lock_or_recover(&data).set_virtual_table_columns(&subq.alias, columns);
        }
    }

    let mut guard = lock_or_recover(&data);
    let suggestions = guard.get_column_suggestions("", Some(&column_tables));
    assert!(
        suggestions
            .iter()
            .any(|c| c.eq_ignore_ascii_case("item_id")),
        "expected item_id suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|c| c == r#""Item Id""#),
        "expected quoted Item Id suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions
            .iter()
            .any(|c| c.eq_ignore_ascii_case("item_nm")),
        "expected item_nm suggestion, got: {:?}",
        suggestions
    );
    assert!(
        suggestions
            .iter()
            .any(|c| c.eq_ignore_ascii_case("item_qty")),
        "expected item_qty suggestion, got: {:?}",
        suggestions
    );
}

#[test]
fn cte_chain_qualified_column_suggestions_include_wildcard_expansion() {
    let sql_with_cursor = r#"
WITH
  base AS (
    SELECT e.empno, e.ename, e.job, e.deptno, e.sal,
           REGEXP_REPLACE(e.ename, '[AEIOU]', '*') AS masked_name
    FROM oqt_t_emp e
  ),
  enriched AS (
    SELECT
      b.*,
      (SELECT d.dname FROM oqt_t_dept d WHERE d.deptno = b.deptno) AS dname,
      NTILE(3) OVER (PARTITION BY b.deptno ORDER BY b.sal DESC) AS sal_band
    FROM base b
  ),
  filtered AS (
    SELECT *
    FROM enriched
    WHERE (sal > (SELECT AVG(sal) FROM oqt_t_emp WHERE deptno = enriched.deptno))
       OR (job IN ('MANAGER','ANALYST') AND sal >= 2500)
  )
SELECT
  f.|,
  f.dname,
  f.empno,
  f.ename,
  f.masked_name,
  f.job,
  f.sal,
  f.sal_band,
  -- window frame with last_value (needs careful frame)
  LAST_VALUE(f.sal) OVER (
    PARTITION BY f.deptno
    ORDER BY f.sal
    ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
  ) AS max_sal_via_last_value
FROM filtered f
ORDER BY f.deptno, f.sal DESC, f.empno;
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let column_tables =
        intellisense_context::resolve_qualifier_tables("f", &deep_ctx.tables_in_scope);
    assert_eq!(
        column_tables,
        vec!["filtered".to_string()],
        "qualifier should resolve to filtered CTE alias"
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();

    for cte in &deep_ctx.ctes {
        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            cte.body_range,
        );
        let mut columns = if !cte.explicit_columns.is_empty() {
            cte.explicit_columns.clone()
        } else if !cte.body_range.is_empty() {
            intellisense_context::extract_select_list_columns(body_tokens)
        } else {
            Vec::new()
        };
        if cte.explicit_columns.is_empty() && !cte.body_range.is_empty() {
            let body_tables_in_scope =
                intellisense_context::collect_tables_in_statement(body_tokens);
            let (wildcard_columns, _wildcard_tables) =
                SqlEditorWidget::expand_virtual_table_wildcards(
                    body_tokens,
                    &body_tables_in_scope,
                    &HashMap::new(),
                    &data,
                    &sender,
                    &connection,
                );
            columns.extend(wildcard_columns);
        }
        SqlEditorWidget::dedup_column_names_case_insensitive(&mut columns);
        if !columns.is_empty() {
            lock_or_recover(&data).set_virtual_table_columns(&cte.name, columns);
        }
    }

    let mut guard = lock_or_recover(&data);
    let suggestions = guard.get_column_suggestions("", Some(&column_tables));

    assert!(
        suggestions.iter().any(|c| c.eq_ignore_ascii_case("EMPNO")),
        "expected EMPNO in suggestions: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|c| c.eq_ignore_ascii_case("DNAME")),
        "expected DNAME in suggestions: {:?}",
        suggestions
    );
    assert!(
        suggestions
            .iter()
            .any(|c| c.eq_ignore_ascii_case("SAL_BAND")),
        "expected SAL_BAND in suggestions: {:?}",
        suggestions
    );
}

#[test]
fn aliased_cte_qualified_wildcard_expands_virtual_projection_for_completion() {
    let sql = r#"
WITH src AS (
    SELECT empno employee_id, ename employee_name
    FROM oqt_t_emp
)
SELECT s.*
FROM src s
"#;

    let token_spans = super::query_text::tokenize_sql_spanned(sql);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, full_tokens.len());

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_ctes(&deep_ctx, &data, &sender, &connection);

    let (columns, wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
        deep_ctx.statement_tokens.as_ref(),
        &deep_ctx.tables_in_scope,
        &virtual_table_columns,
        &data,
        &sender,
        &connection,
    );

    assert_eq!(wildcard_tables, vec!["src".to_string()]);
    assert_has_case_insensitive(&columns, "employee_id");
    assert_has_case_insensitive(&columns, "employee_name");
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_include_generated_columns() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (SUM(sal) FOR job IN ('CLERK' AS clerk_sal)) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));
    assert!(
        suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("clerk_sal")),
        "expected generated pivot alias in qualified suggestions, got: {:?}",
        suggestions
    );
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_include_aggregate_alias_combinations() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) AS total_sal,
  COUNT(*) AS row_count
  FOR job IN ('CLERK' AS clerk, 'MANAGER' AS manager)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    for expected in [
        "clerk_total_sal",
        "clerk_row_count",
        "manager_total_sal",
        "manager_row_count",
    ] {
        assert!(
            suggestions
                .iter()
                .any(|column| column.eq_ignore_ascii_case(expected)),
            "expected generated pivot alias `{expected}` in qualified suggestions, got: {:?}",
            suggestions
        );
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "pivot source column `{unexpected}` should not be exposed as an output column: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_include_aggregate_alias_without_as() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) total_sal,
  COUNT(*) row_count
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    assert_has_case_insensitive(&suggestions, "clerk_row_count");
    for unexpected in ["job", "sal", "clerk"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate aliases without AS should drive output names, got: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_handle_multi_column_for_values() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, quarter_key, region_key, sales_amt FROM sales_fact)
PIVOT (
  SUM(sales_amt) AS total_sales
  FOR (quarter_key, region_key) IN (('Q1', 'N') AS q1_n, ('Q2', 'S') AS q2_s)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "q1_n_total_sales");
    assert_has_case_insensitive(&suggestions, "q2_s_total_sales");
    for unexpected in ["quarter_key", "region_key", "sales_amt"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "multi-column PIVOT should not expose source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_remove_all_aggregate_expression_inputs() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, quarter_key, sales_amt, discount_amt FROM sales_fact)
PIVOT (
  SUM(sales_amt + discount_amt) AS total_sales
  FOR quarter_key IN ('Q1' AS q1)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "q1_total_sales");
    for unexpected in ["quarter_key", "sales_amt", "discount_amt"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate expression should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_keep_grouping_column_named_like_keep_keyword() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, last FROM oqt_t_emp)
PIVOT (
  SUM(sal) KEEP (DENSE_RANK LAST ORDER BY hiredate) AS top_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "last");
    assert_has_case_insensitive(&suggestions, "clerk_top_sal");
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT KEEP output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_keep_dense_rank_first_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, dense_rank, first, nulls FROM oqt_t_emp)
PIVOT (
  SUM(sal) KEEP (DENSE_RANK FIRST ORDER BY hiredate NULLS FIRST) AS top_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "dense_rank", "first", "nulls", "clerk_top_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT KEEP DENSE_RANK FIRST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_quoted_aggregate_input_columns_named_like_keep_keywords(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "first", "last" FROM oqt_t_emp)
PIVOT (
  SUM("first" + "last") AS total_rank
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_rank");
    for unexpected in ["job", r#""first""#, r#""last""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named like KEEP keywords should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_keep_grouping_column_named_like_bind() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, adjustment FROM oqt_t_emp)
PIVOT (
  SUM(sal + :adjustment) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "adjustment");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT bind expression output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_keep_grouping_column_named_like_argument() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, expr FROM oqt_t_emp)
PIVOT (
  SUM(NVL2(expr => sal, value1 => sal, value2 => 0)) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "expr");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT named-argument expression output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_keep_grouping_columns_named_like_aggregate_modifiers(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, all, unique FROM oqt_t_emp)
PIVOT (
  COUNT(ALL sal) AS all_cnt,
  COUNT(UNIQUE sal) AS unique_cnt
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "all",
        "unique",
        "clerk_all_cnt",
        "clerk_unique_cnt",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate modifier output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_filter_where_keeps_grouping_column_named_where()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, status, filter, where FROM oqt_t_emp)
PIVOT (
  SUM(sal) FILTER (WHERE status = 'Y') AS active_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "filter");
    assert_has_case_insensitive(&suggestions, "where");
    assert_has_case_insensitive(&suggestions, "clerk_active_sal");
    for unexpected in ["job", "sal", "status"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT FILTER output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_column_named_filter() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "filter" FROM oqt_t_emp)
PIVOT (
  SUM("filter") AS total_filter
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_filter");
    for unexpected in ["job", r#""filter""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named FILTER should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_filter_where_removes_condition_column_named_filter(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, "filter" FROM oqt_t_emp)
PIVOT (
  SUM(sal) FILTER (WHERE "filter" = 'Y') AS active_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_active_sal");
    for unexpected in ["job", "sal", r#""filter""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT FILTER condition named FILTER should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_listagg_overflow_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, overflow, truncate, with, count FROM oqt_t_emp)
PIVOT (
  LISTAGG(name_col, ',' ON OVERFLOW TRUNCATE '...' WITH COUNT) AS names
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "overflow",
        "truncate",
        "with",
        "count",
        "clerk_names",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT LISTAGG overflow output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_listagg_within_group_removes_order_column() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, sort_col, within, group_col FROM oqt_t_emp)
PIVOT (
  LISTAGG(name_col, ',') WITHIN GROUP (ORDER BY sort_col DESC NULLS LAST) AS names
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "within", "group_col", "clerk_names"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "sort_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT LISTAGG WITHIN GROUP output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_percentile_cont_within_group_removes_order_column(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, within, group_col FROM oqt_t_emp)
PIVOT (
  PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY sal) AS median_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "within", "group_col", "clerk_median_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT PERCENTILE_CONT WITHIN GROUP output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_hypothetical_rank_within_group_removes_input_columns(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, target_sal, sal, within, group_col FROM oqt_t_emp)
PIVOT (
  RANK(target_sal) WITHIN GROUP (ORDER BY sal) AS sal_rank
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "within", "group_col", "clerk_sal_rank"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "target_sal", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT hypothetical RANK WITHIN GROUP output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_window_exclude_ties_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, "exclude", "ties" FROM oqt_t_emp)
PIVOT (
  MAX(SUM(sal) OVER (ORDER BY sal ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE TIES)) AS running_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""exclude""#, r#""ties""#, "clerk_running_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT window EXCLUDE TIES output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_column_named_over() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "over" FROM oqt_t_emp)
PIVOT (
  SUM("over") AS total_over
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_over");
    for unexpected in ["job", r#""over""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named OVER should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_window_groups_exclude_no_others_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, "groups", "no", "others" FROM oqt_t_emp)
PIVOT (
  MAX(SUM(sal) OVER (ORDER BY sal GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE NO OTHERS)) AS running_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        r#""groups""#,
        r#""no""#,
        r#""others""#,
        "clerk_running_sal",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT window GROUPS EXCLUDE NO OTHERS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_analytic_ignore_nulls_keeps_grouping_column_named_ignore(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, "ignore" FROM oqt_t_emp)
PIVOT (
  MAX(FIRST_VALUE(sal) IGNORE NULLS OVER (ORDER BY hiredate)) AS first_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""ignore""#, "clerk_first_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT analytic IGNORE NULLS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_analytic_respect_nulls_keeps_grouping_column_named_respect(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, "respect" FROM oqt_t_emp)
PIVOT (
  MAX(LAST_VALUE(sal) RESPECT NULLS OVER (ORDER BY hiredate)) AS last_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""respect""#, "clerk_last_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT analytic RESPECT NULLS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_nth_value_from_last_keeps_grouping_column_named_from(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, "from" FROM oqt_t_emp)
PIVOT (
  MAX(NTH_VALUE(sal, 1) FROM LAST IGNORE NULLS OVER (ORDER BY hiredate)) AS last_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, "clerk_last_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT NTH_VALUE FROM LAST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_nth_value_nested_arg_from_last_keeps_grouping_column_named_from(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, "from" FROM oqt_t_emp)
PIVOT (
  MAX(NTH_VALUE(NVL(sal, 0), 1) FROM LAST IGNORE NULLS OVER (ORDER BY hiredate)) AS last_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, "clerk_last_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT NTH_VALUE nested arg FROM LAST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_nth_value_from_first_keeps_grouping_column_named_from(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, "from" FROM oqt_t_emp)
PIVOT (
  MAX(NTH_VALUE(sal, 1) FROM FIRST RESPECT NULLS OVER (ORDER BY hiredate)) AS first_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, "clerk_first_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT NTH_VALUE FROM FIRST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_column_named_ignore() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "ignore" FROM oqt_t_emp)
PIVOT (
  SUM("ignore") AS total_ignore
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_ignore");
    for unexpected in ["job", r#""ignore""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named IGNORE should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_column_named_respect() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "respect" FROM oqt_t_emp)
PIVOT (
  SUM("respect") AS total_respect
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_respect");
    for unexpected in ["job", r#""respect""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named RESPECT should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_column_named_from() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "from" FROM oqt_t_emp)
PIVOT (
  SUM("from") AS total_from
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_from");
    for unexpected in ["job", r#""from""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named FROM should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_removes_aggregate_input_columns_named_order_by()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, "order", "by" FROM oqt_t_emp)
PIVOT (
  SUM("order" + "by") AS total_order
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_order");
    for unexpected in ["job", r#""order""#, r#""by""#] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT aggregate input named ORDER/BY should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_type_name_keeps_grouping_column_named_number(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, number FROM oqt_t_emp)
PIVOT (
  SUM(CAST(sal AS NUMBER)) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "number");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_length_semantics_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "char", "byte" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(name_col AS VARCHAR2(10 CHAR))) AS name_text
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""char""#, r#""byte""#, "clerk_name_text"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST length semantics output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_byte_semantics_keeps_grouping_column_named_byte(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "byte" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(name_col AS VARCHAR2(10 BYTE))) AS name_text
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""byte""#, "clerk_name_text"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST BYTE semantics output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_character_set_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "set", utf8mb4 FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(name_col AS CHAR CHARACTER SET utf8mb4)) AS name_text
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""set""#, "utf8mb4", "clerk_name_text"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST CHARACTER SET output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_character_varying_keeps_grouping_column_named_varying(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "varying" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(name_col AS CHARACTER VARYING(30))) AS name_text
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""varying""#, "clerk_name_text"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST CHARACTER VARYING output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_national_character_keeps_grouping_column_named_national(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "national" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(name_col AS NATIONAL CHARACTER VARYING(30))) AS name_text
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""national""#, "clerk_name_text"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST NATIONAL CHARACTER output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_unsigned_keeps_grouping_column_named_unsigned(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, "unsigned" FROM oqt_t_emp)
PIVOT (
  SUM(CAST(sal AS UNSIGNED INTEGER)) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""unsigned""#, "clerk_total_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST UNSIGNED output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_urowid_keeps_grouping_column_named_urowid()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, rowid_col, "urowid" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(rowid_col AS UROWID)) AS rowid_count
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""urowid""#, "clerk_rowid_count"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "rowid_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST UROWID output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_rowid_keeps_grouping_column_named_rowid() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, rowid_col, "rowid" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(rowid_col AS ROWID)) AS rowid_count
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""rowid""#, "clerk_rowid_count"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "rowid_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST ROWID output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_timestamp_without_time_zone_keeps_grouping_column_named_without(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, ts_col, "without" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(ts_col AS TIMESTAMP WITHOUT TIME ZONE)) AS ts_count
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""without""#, "clerk_ts_count"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "ts_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST TIMESTAMP WITHOUT TIME ZONE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlcast_type_name_keeps_grouping_column_named_number(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, number FROM oqt_t_emp)
PIVOT (
  SUM(XMLCAST(payload_xml AS NUMBER)) AS total_amt
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "number");
    assert_has_case_insensitive(&suggestions, "clerk_total_amt");
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLCAST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_treat_type_name_keeps_grouping_column_named_like_type(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, employee_t FROM oqt_t_emp)
PIVOT (
  COUNT(TREAT(obj_col AS employee_t)) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "employee_t");
    assert_has_case_insensitive(&suggestions, "clerk_typed_obj");
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT TREAT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_treat_qualified_type_keeps_grouping_columns_named_like_type_path(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, app_schema, employee_t FROM oqt_t_emp)
PIVOT (
  COUNT(TREAT(obj_col AS app_schema.employee_t)) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "app_schema", "employee_t", "clerk_typed_obj"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT TREAT qualified type output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_treat_quoted_qualified_type_keeps_grouping_columns_named_like_type_path(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, "App Schema", "Employee Type" FROM oqt_t_emp)
PIVOT (
  COUNT(TREAT(obj_col AS "App Schema"."Employee Type")) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        r#""App Schema""#,
        r#""Employee Type""#,
        "clerk_typed_obj",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT TREAT quoted qualified type output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_user_type_keeps_grouping_column_named_like_type(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, employee_t FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(obj_col AS employee_t)) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "employee_t");
    assert_has_case_insensitive(&suggestions, "clerk_typed_obj");
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST user type output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_qualified_user_type_keeps_grouping_columns_named_like_type_path(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, app_schema, employee_t FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(obj_col AS app_schema.employee_t)) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "app_schema", "employee_t", "clerk_typed_obj"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST qualified user type output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_cast_quoted_qualified_user_type_keeps_grouping_columns_named_like_type_path(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, "App Schema", "Employee Type" FROM oqt_t_emp)
PIVOT (
  COUNT(CAST(obj_col AS "App Schema"."Employee Type")) AS typed_obj
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        r#""App Schema""#,
        r#""Employee Type""#,
        "clerk_typed_obj",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "obj_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT CAST quoted qualified user type output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlquery_syntax_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, passing, returning, content FROM oqt_t_emp)
PIVOT (
  COUNT(XMLQUERY('/root' PASSING payload_xml RETURNING CONTENT)) AS xml_count
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "passing", "returning", "content", "clerk_xml_count"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLQUERY output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlserialize_syntax_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, content, clob, indent, size FROM oqt_t_emp)
PIVOT (
  COUNT(XMLSERIALIZE(CONTENT payload_xml AS CLOB INDENT SIZE = 2)) AS xml_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "content",
        "clob",
        "indent",
        "size",
        "clerk_xml_doc",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLSERIALIZE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlserialize_removes_input_column_named_content(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, content FROM oqt_t_emp)
PIVOT (
  COUNT(XMLSERIALIZE(CONTENT content AS CLOB)) AS xml_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "clerk_xml_doc"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "content"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLSERIALIZE input output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlelement_name_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, name, employee FROM oqt_t_emp)
PIVOT (
  COUNT(XMLELEMENT(NAME employee, payload_xml)) AS xml_elem
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "name", "employee", "clerk_xml_elem"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLELEMENT NAME output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlelement_evalname_removes_name_expression_column(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, employee, payload_xml, evalname FROM oqt_t_emp)
PIVOT (
  COUNT(XMLELEMENT(EVALNAME employee, payload_xml)) AS xml_elem
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "evalname", "clerk_xml_elem"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "employee", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLELEMENT EVALNAME output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlelement_escaping_name_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, entityescaping, name, employee FROM oqt_t_emp)
PIVOT (
  COUNT(XMLELEMENT(ENTITYESCAPING NAME employee, payload_xml)) AS xml_elem
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "entityescaping",
        "name",
        "employee",
        "clerk_xml_elem",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLELEMENT escaping NAME output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlforest_alias_keeps_grouping_column_named_like_alias(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, payload_doc FROM oqt_t_emp)
PIVOT (
  COUNT(XMLFOREST(payload_xml AS payload_doc)) AS xml_forest
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "payload_doc", "clerk_xml_forest"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLFOREST output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlagg_order_by_removes_order_column() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, sort_col, "order", "by" FROM oqt_t_emp)
PIVOT (
  COUNT(XMLAGG(XMLELEMENT(NAME item, payload_xml) ORDER BY sort_col)) AS xml_items
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""order""#, r#""by""#, "clerk_xml_items"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml", "sort_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLAGG ORDER BY output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlforest_escaping_alias_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, nonentityescaping, payload_doc FROM oqt_t_emp)
PIVOT (
  COUNT(XMLFOREST(NONENTITYESCAPING payload_xml AS payload_doc)) AS xml_forest
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "nonentityescaping",
        "payload_doc",
        "clerk_xml_forest",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLFOREST escaping output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlforest_removes_input_column_named_entityescaping(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, entityescaping, attr_name FROM oqt_t_emp)
PIVOT (
  COUNT(XMLFOREST(entityescaping AS attr_name)) AS xml_forest
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "attr_name", "clerk_xml_forest"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "entityescaping"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLFOREST input output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlpi_name_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, name, employee FROM oqt_t_emp)
PIVOT (
  COUNT(XMLPI(NAME employee, payload_xml)) AS xml_pi
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "name", "employee", "clerk_xml_pi"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLPI NAME output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlpi_removes_value_expression_column() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, employee, payload_xml FROM oqt_t_emp)
PIVOT (
  COUNT(XMLPI(NAME employee, payload_xml)) AS xml_pi
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "employee", "clerk_xml_pi"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLPI value output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlroot_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, version, standalone, yes FROM oqt_t_emp)
PIVOT (
  COUNT(XMLROOT(payload_xml, VERSION '1.0', STANDALONE YES)) AS rooted_xml
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "version",
        "standalone",
        "yes",
        "clerk_rooted_xml",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLROOT options output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlroot_removes_version_expression_column() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, version FROM oqt_t_emp)
PIVOT (
  COUNT(XMLROOT(payload_xml, VERSION version)) AS rooted_xml
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "clerk_rooted_xml"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml", "version"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLROOT value output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlparse_syntax_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, content, wellformed FROM oqt_t_emp)
PIVOT (
  COUNT(XMLPARSE(CONTENT payload_xml WELLFORMED)) AS parsed_xml
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "content", "wellformed", "clerk_parsed_xml"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLPARSE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlparse_removes_input_column_named_content() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, content FROM oqt_t_emp)
PIVOT (
  COUNT(XMLPARSE(CONTENT content)) AS parsed_xml
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "clerk_parsed_xml"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "content"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLPARSE input output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_xmlexists_syntax_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload_xml, passing, by, value, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN XMLEXISTS('/root' PASSING BY VALUE payload_xml) THEN sal ELSE 0 END) AS xml_exists_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "passing",
        "by",
        "value",
        "clerk_xml_exists_sal",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload_xml", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT XMLEXISTS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_extract_field_keeps_grouping_column_named_year()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, hiredate, year FROM oqt_t_emp)
PIVOT (
  SUM(EXTRACT(YEAR FROM hiredate)) AS hire_year
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "year");
    assert_has_case_insensitive(&suggestions, "clerk_hire_year");
    for unexpected in ["job", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT EXTRACT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_trim_spec_keeps_grouping_column_named_leading()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, code, leading FROM oqt_t_emp)
PIVOT (
  MAX(TRIM(LEADING '0' FROM code)) AS clean_code
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "leading");
    assert_has_case_insensitive(&suggestions, "clerk_clean_code");
    for unexpected in ["job", "code"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT TRIM output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_date_literal_keeps_grouping_column_named_date()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, date FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN hiredate >= DATE '2024-01-01' THEN sal ELSE 0 END) AS recent_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "date");
    assert_has_case_insensitive(&suggestions, "clerk_recent_sal");
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT date literal output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_substring_from_for_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, start_pos, len_col, "from", "for" FROM oqt_t_emp)
PIVOT (
  MAX(SUBSTRING(name_col FROM start_pos FOR len_col)) AS part_name
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, r#""for""#, "clerk_part_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "start_pos", "len_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT SUBSTRING FROM/FOR output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_overlay_syntax_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, repl_col, start_pos, len_col, placing, "from", "for" FROM oqt_t_emp)
PIVOT (
  MAX(OVERLAY(name_col PLACING repl_col FROM start_pos FOR len_col)) AS masked_name
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "placing",
        r#""from""#,
        r#""for""#,
        "clerk_masked_name",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "repl_col", "start_pos", "len_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT OVERLAY output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_position_from_keeps_grouping_column_named_from()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, "from", sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN POSITION('A' FROM name_col) > 0 THEN sal ELSE 0 END) AS matched_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, "clerk_matched_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT POSITION FROM output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_interval_year_to_month_keeps_grouping_column_named_to(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal, hiredate, to FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN hiredate >= hiredate - INTERVAL '1-2' YEAR TO MONTH THEN sal ELSE 0 END) AS recent_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "to");
    assert_has_case_insensitive(&suggestions, "clerk_recent_sal");
    for unexpected in ["job", "sal", "hiredate"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT interval literal output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_at_time_zone_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, created_at, at, time, zone FROM oqt_t_emp)
PIVOT (
  MAX(created_at AT TIME ZONE 'UTC') AS utc_created
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "at", "time", "zone", "clerk_utc_created"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "created_at"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT AT TIME ZONE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_collate_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name, collate, binary_ci FROM oqt_t_emp)
PIVOT (
  MAX(name COLLATE BINARY_CI) AS max_name
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "collate", "binary_ci", "clerk_max_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT COLLATE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_conversion_default_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, amount_txt, default, conversion, error FROM oqt_t_emp)
PIVOT (
  SUM(TO_NUMBER(amount_txt DEFAULT 0 ON CONVERSION ERROR)) AS amount_num
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "default", "conversion", "error", "clerk_amount_num"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "amount_txt"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT conversion DEFAULT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_validate_conversion_keeps_grouping_column_named_number(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, amount_txt, sal, number FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN VALIDATE_CONVERSION(amount_txt AS NUMBER) = 1 THEN sal ELSE 0 END) AS valid_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "number", "clerk_valid_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "amount_txt", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT VALIDATE_CONVERSION output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_query_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, format, json, with, wrapper, pretty FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_QUERY(payload, '$.items' RETURNING CLOB FORMAT JSON WITH WRAPPER PRETTY)) AS item_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "format",
        "json",
        "with",
        "wrapper",
        "pretty",
        "clerk_item_doc",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_QUERY output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_object_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, key_col, val_col, key, value, absent, with, keys, strict FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_OBJECT(KEY key_col VALUE val_col ABSENT ON NULL WITH UNIQUE KEYS STRICT RETURNING CLOB)) AS json_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "deptno",
        "key",
        "value",
        "absent",
        "with",
        "keys",
        "strict",
        "clerk_json_doc",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "key_col", "val_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_OBJECT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_object_removes_input_column_named_key() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, key, val_col FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_OBJECT(key VALUE val_col)) AS json_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "clerk_json_doc"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "key", "val_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_OBJECT input output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_transform_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, status_col, set, returning, clob FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_TRANSFORM(payload, SET '$.status' = status_col RETURNING CLOB)) AS payload_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "set", "returning", "clob", "clerk_payload_doc"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload", "status_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_TRANSFORM output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_transform_operation_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, remove, ignore, missing FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_TRANSFORM(payload, REMOVE '$.old' IGNORE ON MISSING)) AS payload_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "remove", "ignore", "missing", "clerk_payload_doc"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_TRANSFORM operation options output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_transform_create_on_missing_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, status_col, set, create, missing FROM oqt_t_emp)
PIVOT (
  COUNT(JSON_TRANSFORM(payload, SET '$.status' = status_col CREATE ON MISSING)) AS payload_doc
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "set", "create", "missing", "clerk_payload_doc"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload", "status_col"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_TRANSFORM CREATE ON MISSING output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_exists_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, min_amt, sal, passing, true, error FROM oqt_t_emp)
PIVOT (
  SUM(CASE
        WHEN JSON_EXISTS(payload, '$?(@.amount > $min)' PASSING min_amt AS "min" TRUE ON ERROR)
        THEN sal
        ELSE 0
      END) AS matched_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "passing", "true", "error", "clerk_matched_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload", "min_amt", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_EXISTS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_json_keeps_grouping_column_named_json() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, json FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN payload IS JSON THEN 1 ELSE 0 END) AS valid_json
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "json", "clerk_valid_json"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS JSON output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_json_options_keep_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, object, with, keys, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN payload IS JSON OBJECT WITH UNIQUE KEYS THEN sal ELSE 0 END) AS json_object_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "object", "with", "keys", "clerk_json_object_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS JSON options output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_of_type_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, obj_col, sal, of, type, employee_t FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN obj_col IS OF TYPE (employee_t) THEN sal ELSE 0 END) AS typed_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "of", "type", "employee_t", "clerk_typed_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "obj_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS OF TYPE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_member_of_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, elem_col, nested_col, member, of, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN elem_col MEMBER OF nested_col THEN sal ELSE 0 END) AS member_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "member", "of", "clerk_member_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "elem_col", "nested_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT MEMBER OF output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_submultiset_of_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, child_nt, parent_nt, submultiset, of, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN child_nt SUBMULTISET OF parent_nt THEN sal ELSE 0 END) AS subset_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "submultiset", "of", "clerk_subset_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "child_nt", "parent_nt", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT SUBMULTISET OF output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_multiset_except_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, child_nt, parent_nt, multiset, except, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN child_nt MULTISET EXCEPT DISTINCT parent_nt IS EMPTY THEN sal ELSE 0 END) AS diff_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "multiset", "except", "clerk_diff_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "child_nt", "parent_nt", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT MULTISET EXCEPT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_empty_keeps_grouping_column_named_empty() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, nested_col, empty, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN nested_col IS EMPTY THEN sal ELSE 0 END) AS empty_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "empty", "clerk_empty_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "nested_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS EMPTY output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_a_set_keeps_grouping_column_named_set() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, nested_col, set, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN nested_col IS A SET THEN sal ELSE 0 END) AS set_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "set", "clerk_set_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "nested_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS A SET output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_like_escape_keeps_grouping_column_named_escape()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, pattern_col, escape_char, escape, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN name_col LIKE pattern_col ESCAPE escape_char THEN sal ELSE 0 END) AS like_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "escape", "clerk_like_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "pattern_col", "escape_char", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT LIKE ESCAPE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_likec_escape_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, pattern_col, escape_char, likec, escape, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN name_col LIKEC pattern_col ESCAPE escape_char THEN sal ELSE 0 END) AS likec_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "likec", "escape", "clerk_likec_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "pattern_col", "escape_char", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT LIKEC ESCAPE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_sounds_like_keeps_grouping_column_named_sounds()
{
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, name_col, pattern_col, sounds, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN name_col SOUNDS LIKE pattern_col THEN sal ELSE 0 END) AS sounds_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "sounds", "clerk_sounds_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "name_col", "pattern_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT SOUNDS LIKE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_like_removes_input_column_named_sounds() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sounds, pattern_col, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN sounds LIKE pattern_col THEN sal ELSE 0 END) AS like_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "clerk_like_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "sounds", "pattern_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT LIKE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_not_distinct_from_keeps_grouping_column_named_from(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, old_val, new_val, sal, "from" FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN old_val IS NOT DISTINCT FROM new_val THEN sal ELSE 0 END) AS same_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", r#""from""#, "clerk_same_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "old_val", "new_val", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS NOT DISTINCT FROM output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_nan_keeps_grouping_column_named_nan() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, ratio_col, nan, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN ratio_col IS NAN THEN sal ELSE 0 END) AS nan_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "nan", "clerk_nan_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "ratio_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS NAN output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_infinite_keeps_grouping_column_named_infinite(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, ratio_col, infinite, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN ratio_col IS INFINITE THEN sal ELSE 0 END) AS infinite_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "infinite", "clerk_infinite_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "ratio_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS INFINITE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_true_keeps_grouping_column_named_true() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, flag_col, true, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN flag_col IS TRUE THEN sal ELSE 0 END) AS true_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "true", "clerk_true_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "flag_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS TRUE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_is_unknown_keeps_grouping_column_named_unknown(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, flag_col, unknown, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN flag_col IS UNKNOWN THEN sal ELSE 0 END) AS unknown_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "unknown", "clerk_unknown_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "flag_col", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT IS UNKNOWN output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_overlaps_keeps_grouping_column_named_overlaps() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, start_a, end_a, start_b, end_b, overlaps, sal FROM oqt_t_emp)
PIVOT (
  SUM(CASE WHEN (start_a, end_a) OVERLAPS (start_b, end_b) THEN sal ELSE 0 END) AS overlap_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "overlaps", "clerk_overlap_sal"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "start_a", "end_a", "start_b", "end_b", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT OVERLAPS output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_returning_keeps_grouping_columns_named_like_syntax(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, returning, number, error, empty FROM oqt_t_emp)
PIVOT (
  SUM(JSON_VALUE(payload, '$.amount' RETURNING NUMBER DEFAULT 0 ON ERROR NULL ON EMPTY)) AS json_amount
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "returning");
    assert_has_case_insensitive(&suggestions, "number");
    assert_has_case_insensitive(&suggestions, "error");
    assert_has_case_insensitive(&suggestions, "empty");
    assert_has_case_insensitive(&suggestions, "clerk_json_amount");
    for unexpected in ["job", "payload"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_VALUE output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_json_default_expression_removes_fallback_column(
) {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, payload, fallback_amt, default, error, empty FROM oqt_t_emp)
PIVOT (
  SUM(JSON_VALUE(payload, '$.amount' RETURNING NUMBER DEFAULT fallback_amt ON ERROR NULL ON EMPTY)) AS json_amount
  FOR job IN ('CLERK' AS clerk)
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["deptno", "default", "error", "empty", "clerk_json_amount"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    for unexpected in ["job", "payload", "fallback_amt"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "PIVOT JSON_VALUE DEFAULT output should not expose input column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_unqualified_column_suggestions_use_output_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT |
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
"#,
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["deptno".to_string(), "job".to_string(), "sal".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            !suggestions
                .iter()
                .any(|column| column.eq_ignore_ascii_case(unexpected)),
            "unqualified PIVOT completion should not leak source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_preserve_quoted_generated_columns() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (SUM(sal) FOR job IN ('CLERK' AS "Clerk Sales")) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("Clerk", Some(&column_tables));

    assert_eq!(
        suggestions,
        vec![r#""Clerk Sales""#.to_string()],
        "quoted generated pivot alias should remain insertable: {:?}",
        suggestions
    );
}

#[test]
fn pivot_clause_alias_qualified_column_suggestions_preserve_quoted_aggregate_combinations() {
    let sql_with_cursor = r#"
SELECT
  p.|
FROM (SELECT deptno, quarter_key, sales_amt FROM sales_fact)
PIVOT (
  SUM(sales_amt) AS "Total Sales"
  FOR quarter_key IN ('Q1' AS "Q1 Sales")
) p
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("p"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("Q1", Some(&column_tables));

    assert_eq!(
        suggestions,
        vec![r#""Q1 Sales_Total Sales""#.to_string()],
        "quoted aggregate pivot alias combination should remain insertable: {:?}",
        suggestions
    );
}

#[test]
fn unpivot_alias_qualified_column_suggestions_preserve_quoted_output_columns() {
    let sql_with_cursor = r#"
SELECT
  un.|
FROM sales_half
UNPIVOT (("sales amount") FOR "quarter tag" IN (h1_sales AS 'H1')) un
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("un"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert!(
        suggestions.iter().any(|column| column == r#""sales amount""#),
        "quoted UNPIVOT measure should remain insertable: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|column| column == r#""quarter tag""#),
        "quoted UNPIVOT FOR column should remain insertable: {:?}",
        suggestions
    );
}

#[test]
fn unpivot_alias_qualified_column_suggestions_hide_quoted_source_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT un.|
FROM sales_half
UNPIVOT (amount FOR metric IN ("H1 Sales" AS 'H1', "H2 Sales" AS 'H2')) un
"#,
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "sales_half",
            vec![r#""H1 Sales""#.to_string(), r#""H2 Sales""#.to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("un"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "amount");
    assert_has_case_insensitive(&suggestions, "metric");
    for unexpected in [r#""H1 Sales""#, r#""H2 Sales""#] {
        assert!(
            suggestions.iter().all(|column| column != unexpected),
            "UNPIVOT output should not expose source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn match_recognize_alias_qualified_column_suggestions_include_generated_columns() {
    let sql_with_cursor = r#"
SELECT
  mr.|
FROM oqt_t_emp
MATCH_RECOGNIZE (
  MEASURES
    FIRST(ename) AS start_name,
    LAST(ename) AS end_name
  PATTERN (a b+)
  DEFINE
    b AS b.sal > PREV(b.sal)
) mr
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("mr"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));
    for expected in ["start_name", "end_name", "a", "b"] {
        assert!(
            suggestions
                .iter()
                .any(|column| column.eq_ignore_ascii_case(expected)),
            "expected `{expected}` in qualified MATCH_RECOGNIZE suggestions, got: {:?}",
            suggestions
        );
    }
}

#[test]
fn match_recognize_alias_qualified_column_suggestions_preserve_quoted_pattern_variables() {
    let sql_with_cursor = r#"
SELECT
  mr.|
FROM oqt_t_emp
MATCH_RECOGNIZE (
  PATTERN ("start row" "end row"+)
  SUBSET "row group" = ("start row", "end row")
  DEFINE
    "end row" AS "end row".sal > PREV("end row".sal)
) mr
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (stmt_start, stmt_end) = SqlEditorWidget::statement_bounds_in_text(&sql, cursor);
    let statement_text = sql.get(stmt_start..stmt_end).unwrap_or("");
    let cursor_in_statement = cursor.saturating_sub(stmt_start);
    let token_spans = super::query_text::tokenize_sql_spanned(statement_text);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor_in_statement);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("mr"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));
    for expected in [r#""start row""#, r#""end row""#, r#""row group""#] {
        assert!(
            suggestions.iter().any(|column| column == expected),
            "expected `{expected}` in qualified MATCH_RECOGNIZE suggestions, got: {:?}",
            suggestions
        );
    }
}

#[test]
fn popup_confirm_key_without_selection_does_not_consume_editor_keys() {
    assert!(!SqlEditorWidget::should_consume_popup_confirm_key(
        Key::Tab,
        false,
    ));
    assert!(!SqlEditorWidget::should_consume_popup_confirm_key(
        Key::Enter,
        false,
    ));
    assert!(!SqlEditorWidget::should_consume_popup_confirm_key(
        Key::KPEnter,
        false,
    ));
}

#[test]
fn popup_confirm_key_with_selection_consumes_enter_and_tab() {
    assert!(SqlEditorWidget::should_consume_popup_confirm_key(
        Key::Tab,
        true,
    ));
    assert!(SqlEditorWidget::should_consume_popup_confirm_key(
        Key::Enter,
        true,
    ));
    assert!(SqlEditorWidget::should_consume_popup_confirm_key(
        Key::KPEnter,
        true,
    ));
}

#[test]
fn leading_indent_prefix_returns_leading_spaces_and_tabs_only() {
    assert_eq!(
        SqlEditorWidget::leading_indent_prefix("    select * from dual"),
        "    "
    );
    assert_eq!(
        SqlEditorWidget::leading_indent_prefix("\t\tselect * from dual"),
        "\t\t"
    );
    assert_eq!(
        SqlEditorWidget::leading_indent_prefix("  \t  select"),
        "  \t  "
    );
}

#[test]
fn leading_indent_prefix_stops_at_first_non_indent_byte() {
    assert_eq!(SqlEditorWidget::leading_indent_prefix("select"), "");
    assert_eq!(SqlEditorWidget::leading_indent_prefix("  -- comment"), "  ");
    assert_eq!(SqlEditorWidget::leading_indent_prefix("  가나다"), "  ");
}

#[test]
fn non_whitespace_char_before_cursor_in_text_detects_semicolon_before_cursor_marker() {
    let sql_with_cursor = "select * from help;|";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let ch = SqlEditorWidget::non_whitespace_char_before_cursor_in_text(&sql, cursor);
    assert_eq!(ch, Some(';'));
}

#[test]
fn non_whitespace_char_before_cursor_in_text_skips_whitespace_after_semicolon() {
    let sql_with_cursor = "select * from help;   |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let ch = SqlEditorWidget::non_whitespace_char_before_cursor_in_text(&sql, cursor);
    assert_eq!(ch, Some(';'));
}

#[test]
fn invoke_void_callback_restores_slot_even_when_callback_panics() {
    let calls = Arc::new(Mutex::new(0usize));
    let calls_for_cb = calls.clone();
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut()>>>> =
        Arc::new(Mutex::new(Some(Box::new(move || {
            *lock_or_recover(&calls_for_cb) += 1;
            panic!("expected callback panic");
        }))));

    let invoked = SqlEditorWidget::invoke_void_callback(&callback_slot);

    assert!(invoked);
    assert!(lock_or_recover(&callback_slot).is_some());
    assert_eq!(*lock_or_recover(&calls), 1);
}

#[test]
fn invoke_void_callback_can_run_again_after_panic() {
    let calls = Arc::new(Mutex::new(0usize));
    let calls_for_cb = calls.clone();
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut()>>>> =
        Arc::new(Mutex::new(Some(Box::new(move || {
            let mut count = lock_or_recover(&calls_for_cb);
            *count += 1;
            if *count == 1 {
                panic!("expected first callback panic");
            }
        }))));

    let first_call = SqlEditorWidget::invoke_void_callback(&callback_slot);
    assert!(first_call);
    assert!(lock_or_recover(&callback_slot).is_some());

    let second_call = SqlEditorWidget::invoke_void_callback(&callback_slot);
    assert!(second_call);
    assert_eq!(*lock_or_recover(&calls), 2);
    assert!(lock_or_recover(&callback_slot).is_some());
}

#[test]
fn invoke_void_callback_returns_false_when_slot_is_empty() {
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));

    let invoked = SqlEditorWidget::invoke_void_callback(&callback_slot);

    assert!(!invoked);
    assert!(lock_or_recover(&callback_slot).is_none());
}

#[test]
fn invoke_void_callback_keeps_replaced_callback_when_original_panics() {
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut()>>>> = Arc::new(Mutex::new(None));
    let replacement_ran = Arc::new(Mutex::new(false));
    let replacement_ran_for_cb = replacement_ran.clone();
    let callback_slot_for_cb = callback_slot.clone();

    *lock_or_recover(&callback_slot) = Some(Box::new(move || {
        let replacement_ran_for_replacement = replacement_ran_for_cb.clone();
        *lock_or_recover(&callback_slot_for_cb) = Some(Box::new(move || {
            *lock_or_recover(&replacement_ran_for_replacement) = true;
        }));
        panic!("expected panic after replacement");
    }));

    let first_call = SqlEditorWidget::invoke_void_callback(&callback_slot);
    assert!(first_call);
    assert!(lock_or_recover(&callback_slot).is_some());

    let second_call = SqlEditorWidget::invoke_void_callback(&callback_slot);
    assert!(second_call);
    assert!(*lock_or_recover(&replacement_ran));
}

#[test]
fn invoke_file_drop_callback_restores_slot_even_when_callback_panics() {
    let calls = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
    let calls_for_cb = calls.clone();
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> =
        Arc::new(Mutex::new(Some(Box::new(move |path: PathBuf| {
            lock_or_recover(&calls_for_cb).push(path);
            panic!("expected callback panic");
        }))));

    let expected_path = PathBuf::from("/tmp/panic.sql");
    let invoked = SqlEditorWidget::invoke_file_drop_callback(&callback_slot, expected_path.clone());

    assert!(invoked);
    assert!(lock_or_recover(&callback_slot).is_some());
    assert_eq!(lock_or_recover(&calls).as_slice(), &[expected_path]);
}

#[test]
fn invoke_file_drop_callback_can_run_again_after_panic() {
    let calls = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
    let calls_for_cb = calls.clone();
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> =
        Arc::new(Mutex::new(Some(Box::new(move |path: PathBuf| {
            let mut events = lock_or_recover(&calls_for_cb);
            let should_panic = events.is_empty();
            events.push(path);
            if should_panic {
                panic!("expected first callback panic");
            }
        }))));

    let first_path = PathBuf::from("/tmp/first.sql");
    let second_path = PathBuf::from("/tmp/second.sql");

    let first_call = SqlEditorWidget::invoke_file_drop_callback(&callback_slot, first_path.clone());
    assert!(first_call);
    assert!(lock_or_recover(&callback_slot).is_some());

    let second_call =
        SqlEditorWidget::invoke_file_drop_callback(&callback_slot, second_path.clone());
    assert!(second_call);
    assert!(lock_or_recover(&callback_slot).is_some());
    assert_eq!(
        lock_or_recover(&calls).as_slice(),
        &[first_path, second_path]
    );
}

#[test]
fn invoke_file_drop_callback_returns_false_when_slot_is_empty() {
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> = Arc::new(Mutex::new(None));
    let path = PathBuf::from("/tmp/ignored.sql");

    let invoked = SqlEditorWidget::invoke_file_drop_callback(&callback_slot, path);

    assert!(!invoked);
    assert!(lock_or_recover(&callback_slot).is_none());
}

#[test]
fn invoke_file_drop_callback_keeps_replaced_callback_when_original_panics() {
    let callback_slot: Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>> = Arc::new(Mutex::new(None));
    let captured_paths = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
    let captured_paths_for_cb = captured_paths.clone();
    let callback_slot_for_cb = callback_slot.clone();

    *lock_or_recover(&callback_slot) = Some(Box::new(move |_path: PathBuf| {
        let captured_paths_for_replacement = captured_paths_for_cb.clone();
        *lock_or_recover(&callback_slot_for_cb) = Some(Box::new(move |path: PathBuf| {
            lock_or_recover(&captured_paths_for_replacement).push(path);
        }));
        panic!("expected panic after replacement");
    }));

    let first_path = PathBuf::from("/tmp/first-replace.sql");
    let second_path = PathBuf::from("/tmp/second-replace.sql");

    let first_call = SqlEditorWidget::invoke_file_drop_callback(&callback_slot, first_path);
    assert!(first_call);
    assert!(lock_or_recover(&callback_slot).is_some());

    let second_call =
        SqlEditorWidget::invoke_file_drop_callback(&callback_slot, second_path.clone());
    assert!(second_call);
    assert_eq!(lock_or_recover(&captured_paths).as_slice(), &[second_path]);
}

#[test]
fn classify_intellisense_context_treats_insert_column_list_as_column_context() {
    let sql_with_cursor = "INSERT INTO employees (|) VALUES (1)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert!(
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        "unexpected context for second SELECT list: {:?}",
        context
    );
}

#[test]
fn classify_intellisense_context_treats_insert_all_second_column_list_as_column_context() {
    let sql_with_cursor =
        "INSERT ALL INTO emp_a (id) VALUES (1) INTO emp_b (|) VALUES (2) SELECT 1 FROM dual";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert!(
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        "unexpected context for second SELECT list: {:?}",
        context
    );
}

#[test]
fn classify_intellisense_context_treats_insert_first_second_column_list_as_column_context() {
    let sql_with_cursor = "INSERT FIRST WHEN 1 = 1 THEN INTO emp_a (id) VALUES (1) \
             WHEN 2 = 2 THEN INTO emp_b (|) VALUES (2) SELECT 1 FROM dual";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::InsertColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn insert_column_list_context_ignores_parentheses_after_select_body_starts() {
    let sql_with_cursor =
        "INSERT INTO audit_emp (emp_id) SELECT * FROM (SELECT | FROM oqt_t_emp) src";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();

    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SelectList);
}

#[test]
fn classify_intellisense_context_treats_with_cte_column_list_as_column_context() {
    let sql_with_cursor = "WITH r (|) AS (SELECT node_id FROM oqt_t_tree) SELECT * FROM r";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::CteColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_derived_alias_column_list_as_column_context() {
    let sql_with_cursor = "SELECT * FROM (SELECT empno, ename FROM oqt_t_emp) d(|)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DerivedAliasColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_with_xmlnamespaces_clause_as_general_context() {
    let sql_with_cursor = "WITH XMLNAMESPACES (DEFAULT | 'urn:emp') SELECT value FROM xml_source";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::Initial);
    assert!(deep_ctx.ctes.is_empty(), "ctes: {:?}", deep_ctx.ctes);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::General);
}

#[test]
fn classify_intellisense_context_treats_with_change_tracking_context_clause_as_general_context() {
    let sql_with_cursor = "WITH CHANGE_TRACKING_CONTEXT (| 0x01) SELECT value FROM xml_source";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::Initial);
    assert!(deep_ctx.ctes.is_empty(), "ctes: {:?}", deep_ctx.ctes);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::General);
}

#[test]
fn cte_column_list_completion_prefers_body_projection_columns() {
    let sql_with_cursor = r#"
WITH r (node_id, |) AS (
  SELECT NODE_ID, parent_id, node_name, 1 AS lvl, '/'||node_name AS path
  FROM oqt_t_tree
  WHERE parent_id IS NULL
  UNION ALL
  SELECT t.NODE_ID, t.parent_id, t.node_name, r.lvl + 1,
         r.path || '/' || t.node_name
  FROM oqt_t_tree t
  JOIN r ON t.PARENT_ID = r.node_id
)
SELECT * FROM r
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("r"))
        .expect("expected CTE r");
    assert!(SqlEditorWidget::is_cursor_inside_cte_explicit_column_list(
        &deep_ctx, cte
    ));

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();

    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["node_id", "parent_id", "node_name", "lvl", "path"] {
        assert!(
            columns.iter().any(|col| col.eq_ignore_ascii_case(expected)),
            "expected `{expected}` in CTE explicit-column completion candidates: {:?}",
            columns
        );
    }
}

#[test]
fn cte_virtual_columns_include_match_recognize_generated_columns() {
    let sql_with_cursor = r#"
WITH mr AS (
    SELECT *
    FROM oqt_t_emp
    MATCH_RECOGNIZE (
      MEASURES
        FIRST(ename) AS start_name,
        LAST(ename) AS end_name
      PATTERN (a b+)
      DEFINE
        b AS b.sal > PREV(b.sal)
    )
)
SELECT mr.| FROM mr
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("mr"))
        .expect("expected CTE mr");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["start_name", "end_name", "a", "b"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn cte_explicit_column_list_completion_preserves_quoted_keyword_projection_aliases() {
    let sql_with_cursor = r#"
WITH q(id_alias, |) AS (
  SELECT
    empno AS id,
    ename AS "order",
    deptno AS "group"
  FROM oqt_t_emp
)
SELECT * FROM q
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("q"))
        .expect("expected CTE q");
    assert!(SqlEditorWidget::is_cursor_inside_cte_explicit_column_list(
        &deep_ctx, cte
    ));

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();

    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["id", r#""order""#, r#""group""#] {
        assert!(
            columns.iter().any(|col| col.eq_ignore_ascii_case(expected)),
            "expected `{expected}` in CTE explicit-column completion candidates: {:?}",
            columns
        );
    }
    assert!(
        columns
            .iter()
            .all(|column| !column.eq_ignore_ascii_case("id_alias")),
        "editing CTE explicit-column list should prefer body projection, got: {:?}",
        columns
    );
}

#[test]
fn cte_virtual_columns_include_model_generated_columns() {
    let sql_with_cursor = r#"
WITH md AS (
    SELECT deptno, sum_sal
    FROM (
      SELECT deptno, SUM(sal) AS sum_sal
      FROM oqt_t_emp
      GROUP BY deptno
    )
    MODEL
      DIMENSION BY (deptno)
      MEASURES (sum_sal, 0 AS avg_sal_calc, 0 AS "Avg Sal")
      RULES (
        avg_sal_calc[ANY] = sum_sal[CV()] / 2,
        "Avg Sal"[ANY] = sum_sal[CV()] + 100
      )
)
SELECT md.| FROM md
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("md"))
        .expect("expected CTE md");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["avg_sal_calc"] {
        assert_has_case_insensitive(&columns, expected);
    }
    assert!(
        columns.iter().any(|column| column == r#""Avg Sal""#),
        "expected quoted MODEL measure alias, got: {:?}",
        columns
    );
}

#[test]
fn cte_virtual_columns_include_recursive_search_and_cycle_generated_columns() {
    let sql_with_cursor = r#"
WITH t(n) AS (
    SELECT 1 AS n
    FROM dual
    UNION ALL
    SELECT n + 1
    FROM t
    WHERE n < 3
)
SEARCH DEPTH FIRST BY n SET ord_seq
CYCLE n SET is_cycle TO 'Y' DEFAULT 'N'
SELECT t.| FROM t
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("t"))
        .expect("expected CTE t");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    assert_has_case_insensitive(&columns, "n");
    assert_has_case_insensitive(&columns, "ord_seq");
    assert_has_case_insensitive(&columns, "is_cycle");
}

#[test]
fn cte_virtual_columns_preserve_quoted_recursive_search_and_cycle_generated_columns() {
    let sql_with_cursor = r#"
WITH t(n) AS (
    SELECT 1 AS n
    FROM dual
    UNION ALL
    SELECT n + 1
    FROM t
    WHERE n < 3
)
SEARCH DEPTH FIRST BY n SET "ord seq"
CYCLE n SET "is cycle" TO 'Y' DEFAULT 'N'
SELECT t.| FROM t
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("t"))
        .expect("expected CTE t");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    assert!(
        columns.iter().any(|column| column == r#""ord seq""#),
        "quoted SEARCH generated column should remain insertable: {:?}",
        columns
    );
    assert!(
        columns.iter().any(|column| column == r#""is cycle""#),
        "quoted CYCLE generated column should remain insertable: {:?}",
        columns
    );
}

#[test]
fn cte_virtual_columns_include_table_function_columns_for_star_projection() {
    let sql_with_cursor = r#"
WITH jt_cte AS (
    SELECT *
    FROM oqt_t_json src
    CROSS JOIN JSON_TABLE(
      src.payload,
      '$'
      COLUMNS (
        order_id NUMBER PATH '$.order_id',
        skill    VARCHAR2(30) PATH '$.skill'
      )
    ) jt
)
SELECT jt_cte.| FROM jt_cte
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("jt_cte"))
        .expect("expected CTE jt_cte");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["order_id", "skill"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn classify_intellisense_context_treats_model_clause_as_column_context() {
    let sql_with_cursor = "WITH m AS ( \
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
             )";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::ModelClause);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn resolve_column_tables_maps_match_recognize_pattern_variable_to_scope_tables() {
    let sql_with_cursor = r#"
	SELECT *
	FROM oqt_t_emp
MATCH_RECOGNIZE (
  PARTITION BY deptno
  ORDER BY hiredate, empno
  MEASURES
    FIRST(ename) AS start_name,
    LAST(ename) AS end_name
  ONE ROW PER MATCH
  PATTERN (a b+)
  DEFINE
    b AS b.| > PREV(b.sal)
)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("b"), &deep_ctx);
    assert!(
        column_tables
            .iter()
            .any(|table| table.eq_ignore_ascii_case("oqt_t_emp")),
        "pattern variable b should resolve to source tables, got: {:?}",
        column_tables
    );
    assert!(
        !column_tables
            .iter()
            .any(|table| table.eq_ignore_ascii_case("b")),
        "pattern variable should not fall back to raw qualifier table key: {:?}",
        column_tables
    );
}

#[test]
fn resolve_column_tables_for_match_recognize_alias_includes_virtual_alias_before_base_table() {
    let sql_with_cursor = r#"
SELECT mr.|
FROM oqt_t_emp
MATCH_RECOGNIZE (
  MEASURES FIRST(ename) AS start_name
  PATTERN (a)
  DEFINE a AS sal > 0
) mr
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("mr"), &deep_ctx);
    assert_eq!(tables.first().map(String::as_str), Some("mr"));
    assert!(
        tables
            .iter()
            .any(|table| table.eq_ignore_ascii_case("oqt_t_emp")),
        "expected base table to remain available after the virtual alias, got: {:?}",
        tables
    );
}

#[test]
fn merge_derived_columns_includes_model_measure_aliases() {
    let tokens = SqlEditorWidget::tokenize_sql(
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

    let mut derived_columns =
        intellisense_context::extract_oracle_unpivot_generated_columns(&tokens);
    derived_columns.extend(intellisense_context::extract_oracle_model_generated_columns(&tokens));

    let merged = SqlEditorWidget::merge_suggestions_with_derived_columns(
        vec!["deptno".to_string(), "sum_sal".to_string()],
        "",
        derived_columns,
    );

    assert!(
        merged
            .iter()
            .any(|c| c.eq_ignore_ascii_case("avg_sal_calc")),
        "expected avg_sal_calc in merged suggestions, got: {:?}",
        merged
    );
    assert!(
        merged
            .iter()
            .any(|c| c.eq_ignore_ascii_case("sum_plus_100")),
        "expected sum_plus_100 in merged suggestions, got: {:?}",
        merged
    );
}

#[test]
fn merge_derived_columns_includes_match_recognize_measures_aliases() {
    let tokens = SqlEditorWidget::tokenize_sql(
        "SELECT * \
             FROM emp \
             MATCH_RECOGNIZE ( \
               MEASURES FIRST(ename) AS start_name, LAST(ename) AS end_name \
               PATTERN (a b+) \
               DEFINE b AS b.sal > PREV(b.sal) \
             ) mr",
    );

    let derived_columns = intellisense_context::extract_match_recognize_generated_columns(&tokens);
    let merged = SqlEditorWidget::merge_suggestions_with_derived_columns(
        vec!["empno".to_string()],
        "",
        derived_columns,
    );

    assert!(
        merged.iter().any(|c| c.eq_ignore_ascii_case("start_name")),
        "expected start_name in merged suggestions, got: {:?}",
        merged
    );
    assert!(
        merged.iter().any(|c| c.eq_ignore_ascii_case("end_name")),
        "expected end_name in merged suggestions, got: {:?}",
        merged
    );
}

#[test]
fn merge_derived_columns_includes_exact_prefix_match() {
    let merged = SqlEditorWidget::merge_suggestions_with_derived_columns(
        vec!["empno".to_string()],
        "start_name",
        vec!["start_name".to_string(), "end_name".to_string()],
    );

    assert_has_case_insensitive(&merged, "start_name");
}

#[test]
fn merge_derived_columns_matches_quoted_alias_by_unquoted_prefix() {
    let merged = SqlEditorWidget::merge_suggestions_with_derived_columns(
        vec!["empno".to_string()],
        "Order",
        vec![r#""Order Id""#.to_string(), r#""Other Alias""#.to_string()],
    );

    assert_eq!(merged, vec!["empno".to_string(), r#""Order Id""#.to_string()]);
}

#[test]
fn merge_derived_columns_matches_backtick_alias_by_unquoted_prefix() {
    let merged = SqlEditorWidget::merge_suggestions_with_derived_columns(
        vec!["empno".to_string()],
        "Order",
        vec!["`Order Id`".to_string(), "`Other Alias`".to_string()],
    );

    assert_eq!(merged, vec!["empno".to_string(), "`Order Id`".to_string()]);
}

#[test]
fn merge_prioritized_derived_columns_keeps_order_by_alias_before_result_limit() {
    let base: Vec<String> = (0..MAX_MERGED_SUGGESTIONS + 20)
        .map(|idx| format!("TOTAL_BASE_{idx:03}"))
        .collect();

    let merged = SqlEditorWidget::merge_suggestions_with_prioritized_derived_columns(
        base,
        "total",
        vec!["total_due".to_string()],
    );

    assert_eq!(merged.first().map(String::as_str), Some("total_due"));
    assert_eq!(merged.len(), MAX_MERGED_SUGGESTIONS);
}

#[test]
fn collect_derived_columns_for_order_by_includes_select_aliases() {
    let sql_with_cursor = "SELECT \
             oh.order_id, \
             oh.cust_name, \
             oh.order_dt, \
             (SELECT SUM(oi.qty*oi.unit_price) FROM oqt_t_order_item oi WHERE oi.ORDER_ID = oh.order_id) AS amt \
           FROM oqt_t_order_hdr oh \
           ORDER BY \
             (SELECT COUNT(*) FROM oqt_t_order_item oi WHERE oi.order_id = oh.order_id) DESC, \
             | DESC NULLS LAST \
           FETCH FIRST 3 ROWS ONLY";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let derived = SqlEditorWidget::collect_derived_columns_for_context(&deep_ctx);
    assert!(
        derived.iter().any(|c| c.eq_ignore_ascii_case("amt")),
        "expected select-list alias `amt` in derived columns: {:?}",
        derived
    );
}

#[test]
fn collect_derived_columns_for_nested_subquery_order_by_uses_current_projection_only() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT q.inner_empno AS outer_alias \
         FROM ( \
           SELECT e.empno AS inner_empno, e.ename AS inner_name \
           FROM emp e \
           ORDER BY | \
         ) q",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let derived = SqlEditorWidget::collect_derived_columns_for_context(&deep_ctx);
    assert_has_case_insensitive(&derived, "inner_empno");
    assert_has_case_insensitive(&derived, "inner_name");
    assert!(
        !derived
            .iter()
            .any(|c| c.eq_ignore_ascii_case("outer_alias")),
        "outer query alias must not leak into nested subquery ORDER BY: {:?}",
        derived
    );
}

#[test]
fn collect_derived_columns_for_cte_body_order_by_uses_current_cte_projection_only() {
    let deep_ctx = analyze_inline_cursor_sql(
        "WITH detail AS ( \
           SELECT e.empno AS cte_empno, e.ename AS cte_name \
           FROM emp e \
           ORDER BY | \
         ) \
         SELECT detail.cte_empno AS outer_alias \
         FROM detail",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let derived = SqlEditorWidget::collect_derived_columns_for_context(&deep_ctx);
    assert_has_case_insensitive(&derived, "cte_empno");
    assert_has_case_insensitive(&derived, "cte_name");
    assert!(
        !derived
            .iter()
            .any(|c| c.eq_ignore_ascii_case("outer_alias")),
        "outer SELECT alias must not leak into CTE body ORDER BY: {:?}",
        derived
    );
}

#[test]
fn collect_derived_columns_for_analytic_order_by_excludes_select_aliases() {
    let deep_ctx = analyze_inline_cursor_sql(
        "SELECT e.empno AS alias_empno, \
                SUM(e.sal) OVER (PARTITION BY e.deptno ORDER BY |) AS running_sal \
         FROM emp e",
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::OrderByClause
    );

    let derived = SqlEditorWidget::collect_derived_columns_for_context(&deep_ctx);
    assert!(
        !derived
            .iter()
            .any(|c| c.eq_ignore_ascii_case("alias_empno")),
        "analytic ORDER BY must not suggest select-list aliases: {:?}",
        derived
    );
    assert!(
        !derived
            .iter()
            .any(|c| c.eq_ignore_ascii_case("running_sal")),
        "analytic ORDER BY must not suggest sibling analytic aliases: {:?}",
        derived
    );
}

#[test]
fn infer_columns_from_partial_select_qualifier_uses_virtual_table_columns() {
    let sql_with_cursor = r#"
SELECT
  jt.order_id,
  it.|,
  (it.qty * it.price) AS line_amt
FROM oqt_t_json j
CROSS JOIN JSON_TABLE(
  j.payload,
  '$'
  COLUMNS (
    order_id NUMBER PATH '$.order_id',
    NESTED PATH '$.items[*]'
    COLUMNS (
      sku   VARCHAR2(30) PATH '$.sku',
      qty   NUMBER       PATH '$.qty',
      price NUMBER       PATH '$.price'
    )
  )
) jt
CROSS APPLY (
  SELECT jt., jt., jt. FROM dual
) it
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let it_subq = deep_ctx
        .subqueries
        .iter()
        .find(|s| s.alias.eq_ignore_ascii_case("it"))
        .expect("expected apply subquery alias it");
    let body_tokens = intellisense_context::token_range_slice(
        deep_ctx.statement_tokens.as_ref(),
        it_subq.body_range,
    );
    let body_tables_in_scope = intellisense_context::collect_tables_in_statement(body_tokens);

    let mut virtual_table_columns = HashMap::new();
    SqlEditorWidget::insert_virtual_table_columns(
        &mut virtual_table_columns,
        "jt",
        vec![
            "order_id".to_string(),
            "sku".to_string(),
            "qty".to_string(),
            "price".to_string(),
        ],
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let inferred = SqlEditorWidget::infer_columns_from_partial_select_qualifiers(
        body_tokens,
        &body_tables_in_scope,
        &deep_ctx.tables_in_scope,
        &virtual_table_columns,
        &data,
        &sender,
        &connection,
    );

    for expected in ["order_id", "sku", "qty", "price"] {
        assert!(
            inferred.iter().any(|c| c.eq_ignore_ascii_case(expected)),
            "expected inferred column `{expected}` in {:?}",
            inferred
        );
    }
}

#[test]
fn collect_virtual_relation_columns_merge_explicit_aliases_with_partial_qualifier_inference() {
    let sql_with_cursor = r#"
SELECT
  it.|
FROM oqt_t_json j
CROSS JOIN JSON_TABLE(
  j.payload,
  '$'
  COLUMNS (
    order_id NUMBER PATH '$.order_id',
    NESTED PATH '$.items[*]'
    COLUMNS (
      sku   VARCHAR2(30) PATH '$.sku',
      qty   NUMBER       PATH '$.qty',
      price NUMBER       PATH '$.price'
    )
  )
) jt
CROSS APPLY (
  SELECT
    jt.,
    (jt.qty * jt.price) AS line_amt
  FROM dual
) it
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("it"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["order_id", "sku", "qty", "price", "line_amt"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn json_table_alias_qualified_completion_includes_for_ordinality_columns() {
    let sql_with_cursor = r#"
SELECT
  jt.|
FROM oqt_t_json j
CROSS JOIN JSON_TABLE(
  j.payload,
  '$'
  COLUMNS (
    order_id NUMBER PATH '$.order_id',
    order_pos FOR ORDINALITY,
    NESTED PATH '$.items[*]'
    COLUMNS (
      line_no FOR ORDINALITY,
      sku VARCHAR2(30) PATH '$.sku'
    )
  )
) jt
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("jt"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["order_id", "order_pos", "line_no", "sku"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn json_table_alias_qualified_completion_preserves_columns_named_like_options() {
    let sql_with_cursor = r#"
SELECT
  jt.|
FROM oqt_t_json j
CROSS JOIN JSON_TABLE(
  j.payload,
  '$'
  COLUMNS (
    columns VARCHAR2(30) PATH '$.columns',
    path VARCHAR2(30) PATH '$.path',
    format VARCHAR2(30) PATH '$.format',
    wrapper VARCHAR2(30) PATH '$.wrapper',
    "on" VARCHAR2(30) PATH '$.on',
    keep VARCHAR2(30) PATH '$.keep',
    omit VARCHAR2(30) PATH '$.omit',
    quotes VARCHAR2(30) PATH '$.quotes',
    exists EXISTS PATH '$.exists',
    payload nvarchar(max) '$.payload' AS JSON
  )
) jt
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("jt"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "columns",
        "path",
        "format",
        "wrapper",
        r#""on""#,
        "keep",
        "omit",
        "quotes",
        "exists",
        "payload",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    assert!(
        suggestions
            .iter()
            .all(|column| !column.eq_ignore_ascii_case("VARCHAR2")),
        "JSON_TABLE output should not expose datatype token as a column: {:?}",
        suggestions
    );
}

#[test]
fn merge_using_subquery_source_alias_qualified_completion_uses_projection_columns() {
    let sql_with_cursor = r#"
MERGE INTO target_table t
USING (
  SELECT
    s.id AS source_id,
    s.val,
    s.updated_at
  FROM staging_source s
) src
ON (t.id = src.|)
WHEN MATCHED THEN UPDATE SET t.val = src.val
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("src"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["source_id", "val", "updated_at"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn derived_table_alias_column_list_overrides_projection_columns_for_completion() {
    let sql_with_cursor = r#"
SELECT d.|
FROM (
  SELECT empno, ename
  FROM oqt_t_emp
) d(id_alias, "Display Name")
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("d"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "id_alias");
    assert!(
        suggestions.iter().any(|column| column == r#""Display Name""#),
        "expected quoted alias-list column, got: {:?}",
        suggestions
    );
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "projection column should be replaced by alias-list columns: {:?}",
        suggestions
    );
}

#[test]
fn derived_table_alias_column_list_overrides_projection_columns_for_unqualified_completion() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT |
FROM (
  SELECT empno, ename
  FROM oqt_t_emp
) d(id_alias, display_name)
"#,
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "id_alias");
    assert_has_case_insensitive(&suggestions, "display_name");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "unqualified derived alias-list completion should not leak body projection columns: {:?}",
        suggestions
    );
}

#[test]
fn derived_table_alias_column_list_completion_prefers_projection_columns_while_editing_list() {
    let sql_with_cursor = r#"
SELECT *
FROM (
  SELECT empno, ename
  FROM oqt_t_emp
) d(id_alias, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "empno");
    assert_has_case_insensitive(&suggestions, "ename");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("id_alias")),
        "alias-list completion should prefer body projection while editing: {:?}",
        suggestions
    );
}

#[test]
fn base_table_alias_column_list_completion_prefers_source_columns_while_editing_list() {
    let sql_with_cursor = r#"
SELECT *
FROM oqt_t_emp e(emp_id_alias, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DerivedAliasColumnList
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "empno");
    assert_has_case_insensitive(&suggestions, "ename");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("emp_id_alias")),
        "alias-list completion should prefer source table columns while editing: {:?}",
        suggestions
    );
}

#[test]
fn base_table_alias_column_list_overrides_source_columns_for_qualified_completion() {
    let sql_with_cursor = r#"
SELECT e.|
FROM oqt_t_emp e(emp_id_alias, "Emp Name Alias")
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("e"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "emp_id_alias");
    assert!(
        suggestions
            .iter()
            .any(|column| column == r#""Emp Name Alias""#),
        "expected quoted alias-list column, got: {:?}",
        suggestions
    );
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "qualified alias-list completion should not leak source columns: {:?}",
        suggestions
    );
}

#[test]
fn base_table_alias_column_list_overrides_source_columns_for_unqualified_completion() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT |
FROM oqt_t_emp e(emp_id_alias, emp_name_alias)
"#,
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "emp_id_alias");
    assert_has_case_insensitive(&suggestions, "emp_name_alias");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "unqualified alias-list completion should not leak source columns: {:?}",
        suggestions
    );
}

#[test]
fn multiple_base_table_alias_column_lists_contribute_unqualified_completion_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT |
FROM oqt_t_emp e(emp_id_alias, emp_name_alias),
     oqt_t_emp f(manager_id_alias, manager_name_alias)
"#,
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in [
        "emp_id_alias",
        "emp_name_alias",
        "manager_id_alias",
        "manager_name_alias",
    ] {
        assert_has_case_insensitive(&suggestions, expected);
    }
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "multi-alias unqualified completion should not leak physical columns: {:?}",
        suggestions
    );
}

#[test]
fn locking_column_list_uses_alias_column_list_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT *
FROM oqt_t_emp e(emp_id_alias, emp_name_alias)
FOR UPDATE OF |
"#,
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::LockingColumnList
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "emp_id_alias");
    assert_has_case_insensitive(&suggestions, "emp_name_alias");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "locking column list should not leak physical columns from alias-list relation: {:?}",
        suggestions
    );
}

#[test]
fn locking_column_list_uses_pivot_output_columns() {
    let deep_ctx = analyze_inline_cursor_sql(
        r#"
SELECT *
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
FOR UPDATE OF |
"#,
    );

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::LockingColumnList
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["deptno".to_string(), "job".to_string(), "sal".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert_has_case_insensitive(&suggestions, "clerk_total_sal");
    for unexpected in ["job", "sal"] {
        assert!(
            suggestions
                .iter()
                .all(|column| !column.eq_ignore_ascii_case(unexpected)),
            "locking column list over PIVOT should not leak source column `{unexpected}`: {:?}",
            suggestions
        );
    }
}

#[test]
fn base_table_alias_column_list_overrides_source_columns_for_wildcard_expansion() {
    let sql_with_cursor = r#"
SELECT e.*
FROM oqt_t_emp e(emp_id_alias, "Emp Name Alias")
"#;

    let token_spans = super::query_text::tokenize_sql_spanned(sql_with_cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, full_tokens.len());

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);

    let (columns, wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
        deep_ctx.statement_tokens.as_ref(),
        &deep_ctx.tables_in_scope,
        &virtual_table_columns,
        &data,
        &sender,
        &connection,
    );

    assert_eq!(wildcard_tables, vec!["e".to_string()]);
    assert_has_case_insensitive(&columns, "emp_id_alias");
    assert!(
        columns.iter().any(|column| column == r#""Emp Name Alias""#),
        "expected quoted alias-list wildcard column, got: {:?}",
        columns
    );
    assert!(
        !columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "wildcard expansion should not leak source columns: {:?}",
        columns
    );
}

#[test]
fn multiple_base_table_alias_column_lists_expand_each_wildcard_source() {
    let sql = r#"
SELECT *
FROM oqt_t_emp e(emp_id_alias, emp_name_alias),
     oqt_t_emp f(manager_id_alias, manager_name_alias)
"#;

    let token_spans = super::query_text::tokenize_sql_spanned(sql);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, full_tokens.len());

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "ename".to_string(), "deptno".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);

    let (columns, wildcard_tables) = SqlEditorWidget::expand_virtual_table_wildcards(
        deep_ctx.statement_tokens.as_ref(),
        &deep_ctx.tables_in_scope,
        &virtual_table_columns,
        &data,
        &sender,
        &connection,
    );

    assert_eq!(wildcard_tables, vec!["e".to_string(), "f".to_string()]);
    for expected in [
        "emp_id_alias",
        "emp_name_alias",
        "manager_id_alias",
        "manager_name_alias",
    ] {
        assert_has_case_insensitive(&columns, expected);
    }
    assert!(
        !columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case("empno")),
        "multi-alias wildcard expansion should not leak physical columns: {:?}",
        columns
    );
}

#[test]
fn join_using_completion_uses_alias_column_list_columns() {
    let sql_with_cursor = r#"
SELECT *
FROM oqt_t_emp e(emp_id_alias, dept_common)
JOIN oqt_t_dept d(dept_common, dept_name_alias) USING (|)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinUsingColumnList
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["empno".to_string(), "deptno".to_string()],
        );
        guard.set_columns_for_table(
            "oqt_t_dept",
            vec!["deptno".to_string(), "dname".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = {
        let guard = lock_or_recover(&data);
        SqlEditorWidget::collect_common_column_suggestions("", &column_tables, &guard)
    };

    assert_has_case_insensitive(&suggestions, "dept_common");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("deptno")),
        "JOIN USING should use alias-list columns, got: {:?}",
        suggestions
    );
}

#[test]
fn join_using_completion_uses_pivot_output_columns() {
    let sql_with_cursor = r#"
SELECT *
FROM (SELECT deptno, job, sal FROM oqt_t_emp)
PIVOT (
  SUM(sal) AS total_sal
  FOR job IN ('CLERK' AS clerk)
) p
JOIN oqt_t_dept d USING (|)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::JoinUsingColumnList
    );

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.set_columns_for_table(
            "oqt_t_emp",
            vec!["deptno".to_string(), "job".to_string(), "sal".to_string()],
        );
        guard.set_columns_for_table(
            "oqt_t_dept",
            vec!["deptno".to_string(), "dname".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = {
        let guard = lock_or_recover(&data);
        SqlEditorWidget::collect_common_column_suggestions("", &column_tables, &guard)
    };

    assert_has_case_insensitive(&suggestions, "deptno");
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("sal")),
        "JOIN USING over PIVOT should not use aggregate source columns, got: {:?}",
        suggestions
    );
}

#[test]
fn table_function_alias_column_list_provides_virtual_columns_for_completion() {
    let sql_with_cursor = r#"
SELECT r.|
FROM TABLE(get_rows()) r(row_id, "Row Value")
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(Some("r"), &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "row_id");
    assert!(
        suggestions.iter().any(|column| column == r#""Row Value""#),
        "expected quoted table-function alias-list column, got: {:?}",
        suggestions
    );
}

#[test]
fn xmltable_alias_column_list_completion_prefers_columns_clause_while_editing_list() {
    let sql_with_cursor = r#"
SELECT *
FROM XMLTABLE(
  '/root/dept'
  COLUMNS
    deptno NUMBER PATH '@deptno',
    "Dept No" NUMBER PATH '@deptno_text'
) x(alias_deptno, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "deptno");
    assert!(
        suggestions.iter().any(|column| column == r#""Dept No""#),
        "expected quoted XMLTABLE output column while editing alias list, got: {:?}",
        suggestions
    );
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("alias_deptno")),
        "alias-list completion should prefer XMLTABLE output columns while editing: {:?}",
        suggestions
    );
}

#[test]
fn openjson_alias_column_list_completion_prefers_with_clause_while_editing_list() {
    let sql_with_cursor = r#"
SELECT *
FROM orders o
CROSS APPLY OPENJSON(o.payload) WITH (
  item_id int '$.id',
  "Item Id" int '$.itemId'
) oj(alias_item_id, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    assert_has_case_insensitive(&suggestions, "item_id");
    assert!(
        suggestions.iter().any(|column| column == r#""Item Id""#),
        "expected quoted OPENJSON output column while editing alias list, got: {:?}",
        suggestions
    );
    assert!(
        !suggestions
            .iter()
            .any(|column| column.eq_ignore_ascii_case("alias_item_id")),
        "alias-list completion should prefer OPENJSON output columns while editing: {:?}",
        suggestions
    );
}

#[test]
fn openjson_alias_column_list_completion_preserves_bracket_quoted_with_columns() {
    let sql_with_cursor = r#"
SELECT *
FROM orders o
CROSS APPLY OPENJSON(o.payload) WITH (
  [Item Id] int '$.itemId',
  [order] nvarchar(20) '$.order',
  plain_name nvarchar(30) '$.plain'
) oj(alias_item_id, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("", Some(&column_tables));

    for expected in ["[Item Id]", "[order]", "plain_name"] {
        assert_has_case_insensitive(&suggestions, expected);
    }
}

#[test]
fn openjson_alias_column_list_completion_matches_bracket_quoted_columns_by_unquoted_prefix() {
    let sql_with_cursor = r#"
SELECT *
FROM orders o
CROSS APPLY OPENJSON(o.payload) WITH (
  [Item Id] int '$.itemId',
  [order] nvarchar(20) '$.order',
  plain_name nvarchar(30) '$.plain'
) oj(alias_item_id, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("I", Some(&column_tables));

    assert!(
        suggestions.iter().any(|column| column == "[Item Id]"),
        "expected bracket-quoted OPENJSON column to match unquoted prefix, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().all(|column| column != "[order]"),
        "unrelated bracket-quoted OPENJSON column should not match prefix I: {:?}",
        suggestions
    );
}

#[test]
fn openjson_alias_column_list_completion_preserves_escaped_bracket_quoted_with_columns() {
    let sql_with_cursor = r#"
SELECT *
FROM orders o
CROSS APPLY OPENJSON(o.payload) WITH (
  [Item]]Id] int '$.itemId',
  [order]]line] nvarchar(20) '$.order',
  plain_name nvarchar(30) '$.plain'
) oj(alias_item_id, |)
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    lock_or_recover(&data).replace_virtual_table_columns(virtual_table_columns);

    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    let suggestions = lock_or_recover(&data).get_column_suggestions("Item]", Some(&column_tables));

    assert!(
        suggestions.iter().any(|column| column == "[Item]]Id]"),
        "expected escaped bracket OPENJSON column to match unescaped prefix, got: {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().all(|column| column != "[order]]line]"),
        "unrelated escaped bracket OPENJSON column should not match prefix Item]: {:?}",
        suggestions
    );
}

#[test]
fn collect_virtual_relation_columns_include_outer_scope_qualified_wildcards() {
    let sql_with_cursor = r#"
SELECT
  src.|
FROM parent_table p
CROSS APPLY (
  SELECT
    p.*,
    c.child_only
  FROM child_table c
  WHERE c.parent_id = p.id
) src
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["PARENT_TABLE".to_string(), "CHILD_TABLE".to_string()];
        guard.rebuild_indices();
        guard.set_columns_for_table(
            "PARENT_TABLE",
            vec!["ID".to_string(), "PARENT_ONLY".to_string()],
        );
        guard.set_columns_for_table(
            "CHILD_TABLE",
            vec![
                "ID".to_string(),
                "PARENT_ID".to_string(),
                "CHILD_ONLY".to_string(),
            ],
        );
    }

    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    let columns = virtual_columns_for(&virtual_table_columns, "src").clone();

    for expected in ["id", "parent_only", "child_only"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn collect_virtual_relation_columns_include_outer_virtual_scope_qualified_wildcards() {
    let sql_with_cursor = r#"
WITH parent_rows AS (
  SELECT
    p.id,
    p.parent_only
  FROM parent_table p
)
SELECT
  src.|
FROM parent_rows pr
CROSS APPLY (
  SELECT
    pr.*
  FROM dual
) src
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["PARENT_TABLE".to_string()];
        guard.rebuild_indices();
        guard.set_columns_for_table(
            "PARENT_TABLE",
            vec!["ID".to_string(), "PARENT_ONLY".to_string()],
        );
    }

    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let virtual_table_columns =
        collect_virtual_columns_from_relations(&deep_ctx, &data, &sender, &connection);
    let columns = virtual_columns_for(&virtual_table_columns, "src").clone();

    for expected in ["id", "parent_only"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn collect_cte_virtual_columns_merge_explicit_aliases_with_partial_qualifier_inference() {
    let sql_with_cursor = r#"
WITH detail AS (
  SELECT
    e.,
    (e.sal * 12) AS annual_sal
  FROM emp e
)
SELECT detail.| FROM detail
"#;

    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let cte = deep_ctx
        .ctes
        .iter()
        .find(|cte| cte.name.eq_ignore_ascii_case("detail"))
        .expect("expected CTE detail");

    let data = Arc::new(Mutex::new(IntellisenseData::new()));
    {
        let mut guard = lock_or_recover(&data);
        guard.tables = vec!["EMP".to_string()];
        guard.rebuild_indices();
        guard.set_columns_for_table(
            "EMP",
            vec!["EMPNO".to_string(), "ENAME".to_string(), "SAL".to_string()],
        );
    }
    let (sender, _receiver) = mpsc::channel::<ColumnLoadUpdate>();
    let connection = create_shared_connection();
    let (columns, _) = SqlEditorWidget::collect_cte_virtual_columns_for_completion(
        &deep_ctx,
        cte,
        &HashMap::new(),
        &data,
        &sender,
        &connection,
    );

    for expected in ["empno", "ename", "sal", "annual_sal"] {
        assert_has_case_insensitive(&columns, expected);
    }
}

#[test]
fn classify_intellisense_context_keeps_insert_into_target_as_table_context() {
    let sql_with_cursor = "INSERT INTO |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::IntoClause);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::TableName);
}

#[test]
fn classify_intellisense_context_treats_insert_values_expression_as_column_context() {
    let sql_with_cursor = "INSERT INTO target (id) VALUES (|)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::ValuesClause);
    assert!(
        deep_ctx.phase.is_column_context(),
        "phase: {:?}",
        deep_ctx.phase
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_merge_insert_column_list_as_column_context() {
    let sql_with_cursor =
            "MERGE INTO target t USING source s ON (t.id = s.id) WHEN NOT MATCHED THEN INSERT (|) VALUES (s.id)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::MergeInsertColumnList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_merge_update_set_as_column_context() {
    let sql_with_cursor =
            "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.value = |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::SetClause);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_merge_update_set_target_as_column_context() {
    let sql_with_cursor =
        "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::DmlSetTargetList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_merge_delete_where_as_column_context() {
    let sql_with_cursor =
        "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN DELETE WHERE |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert!(
        deep_ctx.phase.is_column_context(),
        "phase: {:?}",
        deep_ctx.phase
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_select_into_target_as_variable_context() {
    let sql_with_cursor = "BEGIN SELECT empno INTO | FROM emp WHERE rownum = 1; END;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectIntoTarget
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::VariableName);
}

#[test]
fn classify_intellisense_context_treats_bulk_collect_into_target_as_variable_context() {
    let sql_with_cursor = "BEGIN SELECT empno BULK COLLECT INTO | FROM emp; END;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::SelectIntoTarget
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::VariableName);
}

#[test]
fn classify_intellisense_context_ignores_prior_select_into_when_cursor_is_in_next_select_list() {
    let sql_with_cursor = "create package body a as
procedure b (c in number) as
begin
select d
into e
from f;
select |
from h;
end;
end;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert!(
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        "unexpected context for second SELECT list: {:?}",
        context
    );
}

#[test]
fn classify_intellisense_context_treats_returning_into_target_as_variable_context() {
    let sql_with_cursor = "UPDATE emp SET sal = sal + 1 RETURNING empno INTO |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::ReturningIntoTarget
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::VariableName);
}

#[test]
fn classify_intellisense_context_treats_fetch_into_target_as_variable_context() {
    let sql_with_cursor = "BEGIN FETCH cur_emp INTO |; END;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::FetchIntoTarget
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::VariableName);
}

#[test]
fn classify_intellisense_context_treats_execute_immediate_using_as_bind_context() {
    let sql_with_cursor = "BEGIN EXECUTE IMMEDIATE 'select count(*) from emp where deptno = :1' INTO l_cnt USING |; END;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::UsingBindList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::BindValue);
}

#[test]
fn classify_intellisense_context_treats_open_for_using_as_bind_context() {
    let sql_with_cursor = "BEGIN OPEN c FOR SELECT empno FROM emp WHERE deptno = :1 USING |; END;";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::UsingBindList
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::BindValue);
}

#[test]
fn classify_intellisense_context_treats_recursive_cte_cycle_set_as_generated_name() {
    let sql_with_cursor =
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) CYCLE n SET | TO 1 DEFAULT 0 SELECT * FROM t";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::RecursiveCteGeneratedColumnName
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::GeneratedName);
}

#[test]
fn classify_intellisense_context_treats_hierarchical_search_set_as_generated_name() {
    let sql_with_cursor =
        "SELECT * FROM emp CONNECT BY PRIOR empno = mgr SEARCH DEPTH FIRST BY empno SET |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::HierarchicalGeneratedColumnName
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::GeneratedName);
}

#[test]
fn classify_intellisense_context_treats_hierarchical_cycle_set_as_generated_name() {
    let sql_with_cursor =
        "SELECT * FROM emp CONNECT BY PRIOR empno = mgr CYCLE empno SET | TO 'Y' DEFAULT 'N'";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert_eq!(
        deep_ctx.phase,
        intellisense_context::SqlPhase::HierarchicalGeneratedColumnName
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::GeneratedName);
}

#[test]
fn generated_name_context_suppresses_completion() {
    assert!(SqlEditorWidget::context_suppresses_completion(
        SqlContext::GeneratedName
    ));
    assert!(!SqlEditorWidget::context_suppresses_completion(
        SqlContext::ColumnName
    ));
}

#[test]
fn classify_intellisense_context_treats_insert_returning_expression_as_column_context() {
    let sql_with_cursor =
        "INSERT INTO emp (empno, ename) VALUES (1, 'ICE') RETURNING | INTO :v_empno";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    assert!(
        deep_ctx.phase.is_column_context(),
        "RETURNING list should be column context, got {:?}",
        deep_ctx.phase
    );

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn resolve_column_tables_for_merge_insert_column_list_prefers_merge_target() {
    let sql_with_cursor =
            "MERGE INTO target_table t USING source_table s ON (t.id = s.id) WHEN NOT MATCHED THEN INSERT (|) VALUES (s.id)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["target_table".to_string()]);
}

#[test]
fn resolve_column_tables_for_insert_all_second_column_list_prefers_current_target() {
    let sql_with_cursor =
        "INSERT ALL INTO emp_a (id) VALUES (1) INTO emp_b (|) VALUES (2) SELECT 1 FROM dual";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["emp_b".to_string()]);
}

#[test]
fn resolve_column_tables_for_insert_first_branch_column_list_prefers_current_target() {
    let sql_with_cursor = "INSERT FIRST WHEN 1 = 1 THEN INTO emp_a (id) VALUES (1) \
             WHEN 2 = 2 THEN INTO emp_b (|) VALUES (2) SELECT 1 FROM dual";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["emp_b".to_string()]);
}

#[test]
fn resolve_column_tables_for_replace_column_list_prefers_target() {
    let sql_with_cursor = "REPLACE INTO audit_emp (|) VALUES (1)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_on_conflict_target_prefers_insert_target() {
    let sql_with_cursor =
            "INSERT INTO audit_emp (emp_id, emp_name) VALUES (1, 'ICE') ON CONFLICT (|) DO UPDATE SET emp_name = EXCLUDED.emp_name";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_on_conflict_excluded_qualifier_maps_to_target() {
    let sql_with_cursor =
            "INSERT INTO audit_emp (emp_id, emp_name) VALUES (1, 'ICE') ON CONFLICT (emp_id) DO UPDATE SET emp_name = EXCLUDED.|";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("EXCLUDED"), &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_insert_returning_prefers_insert_target() {
    let sql_with_cursor = "INSERT INTO audit_emp (emp_id) \
             SELECT e.empno FROM employees e RETURNING | INTO :v_emp_id";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_update_set_prefers_update_target() {
    let sql_with_cursor = "UPDATE audit_emp a SET |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_merge_update_set_prefers_merge_target() {
    let sql_with_cursor = "MERGE INTO target_table t USING source_table s ON (t.id = s.id) \
             WHEN MATCHED THEN UPDATE SET |";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["target_table".to_string()]);
}

#[test]
fn resolve_column_tables_for_join_using_prefers_current_join_operands() {
    let sql_with_cursor = "SELECT * FROM offices o JOIN employees e ON o.office_id = e.office_id \
             JOIN departments d USING (|)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(
        tables,
        vec!["employees".to_string(), "departments".to_string()]
    );
}

#[test]
fn resolve_column_tables_for_join_using_rejects_qualified_name() {
    let sql_with_cursor = "SELECT * FROM employees e JOIN departments d USING (e.|)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("e"), &deep_ctx);
    assert!(tables.is_empty(), "tables: {:?}", tables);
}

#[test]
fn resolve_column_tables_for_recursive_cte_search_by_prefers_recursive_cte() {
    let sql_with_cursor =
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) \
             SEARCH DEPTH FIRST BY | SET ord SELECT * FROM t";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["t".to_string()]);
}

#[test]
fn resolve_column_tables_for_recursive_cte_cycle_prefers_recursive_cte() {
    let sql_with_cursor =
        "WITH t(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM t WHERE n < 3) \
             CYCLE | SET ord TO 1 DEFAULT 0 SELECT * FROM t";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["t".to_string()]);
}

#[test]
fn resolve_column_tables_for_locking_of_prefers_current_query_scope() {
    let sql_with_cursor =
            "SELECT * FROM parent p WHERE EXISTS (SELECT 1 FROM child c WHERE c.parent_id = p.id FOR UPDATE OF |)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["child".to_string()]);
}

#[test]
fn resolve_column_tables_for_correlated_subquery_prefers_current_scope_first() {
    let sql_with_cursor =
        "SELECT * FROM parent_table p WHERE EXISTS (SELECT 1 FROM child_table c WHERE |)";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(
        tables,
        vec!["child_table".to_string(), "parent_table".to_string()]
    );
}

#[test]
fn resolve_column_tables_for_merge_update_set_filters_non_target_qualifier() {
    let sql_with_cursor = "MERGE INTO target_table t USING source_table s ON (t.id = s.id) \
             WHEN MATCHED THEN UPDATE SET s.| = 1";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(Some("s"), &deep_ctx);
    assert!(tables.is_empty(), "tables: {:?}", tables);
}

#[test]
fn collect_common_column_suggestions_for_join_using_intersects_columns() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec![
            "EMPNO".to_string(),
            "DEPTNO".to_string(),
            "LOCATION_ID".to_string(),
        ],
    );
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec![
            "DEPTNO".to_string(),
            "DNAME".to_string(),
            "LOCATION_ID".to_string(),
        ],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_has_case_insensitive(&suggestions, "DEPTNO");
    assert_has_case_insensitive(&suggestions, "LOCATION_ID");
    assert!(
        !suggestions.iter().any(|s| s.eq_ignore_ascii_case("EMPNO")),
        "suggestions: {:?}",
        suggestions
    );
    assert!(
        !suggestions.iter().any(|s| s.eq_ignore_ascii_case("DNAME")),
        "suggestions: {:?}",
        suggestions
    );
}

#[test]
fn collect_common_column_suggestions_include_exact_prefix_match() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table("EMPLOYEES", vec!["EMPNO".to_string(), "DEPTNO".to_string()]);
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec!["DEPTNO".to_string(), "LOCATION_ID".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "deptno",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_has_case_insensitive(&suggestions, "DEPTNO");
}

#[test]
fn collect_common_column_suggestions_match_quoted_columns_by_unquoted_prefix() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec![r#""Dept No""#.to_string(), r#""Emp No""#.to_string()],
    );
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec![r#""Dept No""#.to_string(), r#""Dept Name""#.to_string()],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "Dept",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_eq!(suggestions, vec![r#""Dept No""#.to_string()]);
}

#[test]
fn collect_common_column_suggestions_match_backtick_columns_by_unquoted_prefix() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec!["`Dept No`".to_string(), "`Emp No`".to_string()],
    );
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec!["`Dept No`".to_string(), "`Dept Name`".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "Dept",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_eq!(suggestions, vec!["`Dept No`".to_string()]);
}

#[test]
fn collect_common_column_suggestions_match_bracket_columns_by_unquoted_prefix() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec!["[Dept No]".to_string(), "[Emp No]".to_string()],
    );
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec!["[Dept No]".to_string(), "[Dept Name]".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "Dept",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_eq!(suggestions, vec!["[Dept No]".to_string()]);
}

#[test]
fn collect_common_column_suggestions_dedups_bracket_quote_equivalent_columns() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec!["[Dept]]No]".to_string(), r#""Dept]No""#.to_string()],
    );
    data.set_columns_for_table(
        "DEPARTMENTS",
        vec!["[Dept]]No]".to_string(), "`Dept]Name`".to_string()],
    );

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "Dept]",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_eq!(suggestions, vec!["[Dept]]No]".to_string()]);
}

#[test]
fn collect_common_column_suggestions_dedups_quote_equivalent_columns_after_intersection() {
    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMPLOYEES",
        vec![r#""Dept No""#.to_string(), "`Dept No`".to_string()],
    );
    data.set_columns_for_table("DEPARTMENTS", vec!["`Dept No`".to_string()]);

    let suggestions = SqlEditorWidget::collect_common_column_suggestions(
        "Dept",
        &["EMPLOYEES".to_string(), "DEPARTMENTS".to_string()],
        &data,
    );

    assert_eq!(suggestions, vec![r#""Dept No""#.to_string()]);
}

#[test]
fn resolve_column_tables_for_cte_explicit_column_list_prefers_current_cte() {
    let sql_with_cursor = "WITH r (|) AS (SELECT node_id FROM oqt_t_tree) SELECT * FROM r";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["r".to_string()]);
}

#[test]
fn resolve_column_tables_for_insert_returning_after_log_errors_prefers_insert_target() {
    let sql_with_cursor = "INSERT INTO audit_emp (emp_id) \
             SELECT e.empno FROM employees e \
             LOG ERRORS INTO err$_audit_emp REJECT LIMIT UNLIMITED \
             RETURNING | INTO :v_emp_id";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["audit_emp".to_string()]);
}

#[test]
fn resolve_column_tables_for_merge_returning_prefers_merge_target() {
    let sql_with_cursor = "MERGE INTO target_table t USING source_table s ON (t.id = s.id) \
             WHEN MATCHED THEN UPDATE SET t.val = s.val RETURNING | INTO :v_id";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");

    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let deep_ctx = intellisense_context::analyze_cursor_context(&full_tokens, split_idx);

    let tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
    assert_eq!(tables, vec!["target_table".to_string()]);
}

#[test]
fn extract_select_list_columns_supports_literal_implicit_alias_in_cte() {
    let sql = "SELECT 'Y' flag FROM dual";
    let token_spans = super::query_text::tokenize_sql_spanned(sql);
    let full_tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    let columns = intellisense_context::extract_select_list_columns(&full_tokens);

    assert!(
        columns.iter().any(|col| col.eq_ignore_ascii_case("flag")),
        "expected implicit literal alias in columns: {:?}",
        columns
    );
}

#[test]
fn resolve_qualified_completion_mode_prefers_relation_columns_for_visible_alias() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT e.| FROM emp e");
    let data = IntellisenseData::new();

    let mode = SqlEditorWidget::resolve_qualified_completion_mode(
        "e",
        SqlContext::ColumnOrAll,
        &deep_ctx,
        &data,
    );

    assert_eq!(mode, Some(QualifiedCompletionMode::RelationColumns));
}

#[test]
fn resolve_qualified_completion_mode_matches_quoted_visible_alias() {
    let deep_ctx = analyze_inline_cursor_sql(r#"SELECT "Dept Alias".| FROM emp "Dept Alias""#);
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier(
        r#""Dept Alias""#,
        vec!["RUN_JOB".to_string()],
    );

    let mode = SqlEditorWidget::resolve_qualified_completion_mode(
        r#""Dept Alias""#,
        SqlContext::ColumnOrAll,
        &deep_ctx,
        &data,
    );

    assert_eq!(mode, Some(QualifiedCompletionMode::RelationColumns));
}

#[test]
fn resolve_qualified_completion_mode_uses_schema_relation_members_in_table_context() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT * FROM scott.|");
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier(
        "SCOTT",
        vec!["EMP".to_string(), "DEPT".to_string()],
    );

    let mode =
        SqlEditorWidget::resolve_qualified_completion_mode("scott", SqlContext::TableName, &deep_ctx, &data);

    assert_eq!(mode, Some(QualifiedCompletionMode::RelationMembers));
}

#[test]
fn resolve_qualified_completion_mode_uses_quoted_schema_relation_members() {
    let deep_ctx = analyze_inline_cursor_sql(r#"SELECT * FROM "SCOTT".|"#);
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier(
        "SCOTT",
        vec!["EMP".to_string(), "DEPT".to_string()],
    );

    let mode = SqlEditorWidget::resolve_qualified_completion_mode(
        r#""SCOTT""#,
        SqlContext::TableName,
        &deep_ctx,
        &data,
    );

    assert_eq!(mode, Some(QualifiedCompletionMode::RelationMembers));
}

#[test]
fn resolve_qualified_completion_mode_uses_quoted_dotted_schema_relation_members() {
    let deep_ctx = analyze_inline_cursor_sql(r#"SELECT * FROM "SALES.OPS".|"#);
    let mut data = IntellisenseData::new();
    data.set_relation_members_for_qualifier(
        "SALES.OPS",
        vec!["ORDERS".to_string(), "ORDER_ITEMS".to_string()],
    );

    let mode = SqlEditorWidget::resolve_qualified_completion_mode(
        r#""SALES.OPS""#,
        SqlContext::TableName,
        &deep_ctx,
        &data,
    );
    let suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        r#""SALES.OPS""#,
        "ORDER",
        &deep_ctx,
    );

    assert_eq!(mode, Some(QualifiedCompletionMode::RelationMembers));
    assert_eq!(
        suggestions,
        vec!["ORDERS".to_string(), "ORDER_ITEMS".to_string()]
    );
}

#[test]
fn resolve_qualified_completion_mode_uses_package_members_in_general_context() {
    let deep_ctx = analyze_inline_cursor_sql("BEGIN demo_pkg.|; END;");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier(
        "DEMO_PKG",
        vec!["RUN_JOB".to_string(), "CALC_BONUS".to_string()],
    );

    let mode = SqlEditorWidget::resolve_qualified_completion_mode(
        "demo_pkg",
        SqlContext::General,
        &deep_ctx,
        &data,
    );

    assert_eq!(mode, Some(QualifiedCompletionMode::ObjectMembers));
}

#[test]
fn resolve_qualified_completion_mode_uses_schema_members_for_oracle_object_ddl_contexts() {
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![
            ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
            ("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure)),
            ("EMP_PK".to_string(), Some(QualifiedMemberKind::Index)),
        ],
    );

    for sql in [
        "DROP INDEX scott.|",
        "ALTER TRIGGER scott.|",
        "GRANT EXECUTE ON scott.|",
        "GRANT DEBUG ON scott.|",
        "REVOKE SELECT, INSERT, UPDATE ON scott.|",
    ] {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );
        let mode =
            SqlEditorWidget::resolve_qualified_completion_mode("scott", context, &deep_ctx, &data);

        assert_eq!(
            mode,
            Some(QualifiedCompletionMode::ObjectMembers),
            "expected schema object-member completion for `{sql}`, got {mode:?} in {context:?}"
        );
    }
}

#[test]
fn schema_object_context_prefers_all_members_when_relation_cache_also_exists() {
    let grant_execute_ctx = analyze_inline_cursor_sql("GRANT EXECUTE ON scott.|");
    let grant_select_ctx = analyze_inline_cursor_sql("GRANT SELECT ON scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![
            ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
            ("EMP_VIEW".to_string(), Some(QualifiedMemberKind::View)),
            ("EMP_SEQ".to_string(), Some(QualifiedMemberKind::Sequence)),
            ("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure)),
            ("UTIL_PKG".to_string(), Some(QualifiedMemberKind::Package)),
            ("ADDRESS_T".to_string(), Some(QualifiedMemberKind::Type)),
        ],
    );
    data.set_relation_members_for_qualifier(
        "SCOTT",
        vec!["EMP".to_string(), "EMP_VIEW".to_string()],
    );

    let execute_mode = SqlEditorWidget::resolve_qualified_completion_mode(
        "scott",
        SqlContext::TableName,
        &grant_execute_ctx,
        &data,
    );
    assert_eq!(
        execute_mode,
        Some(QualifiedCompletionMode::ObjectMembers)
    );
    let execute_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_execute_ctx,
    );
    assert_eq!(
        execute_suggestions,
        vec![
            "ADDRESS_T".to_string(),
            "RUN_JOB".to_string(),
            "UTIL_PKG".to_string(),
        ]
    );

    let select_mode = SqlEditorWidget::resolve_qualified_completion_mode(
        "scott",
        SqlContext::TableName,
        &grant_select_ctx,
        &data,
    );
    assert_eq!(select_mode, Some(QualifiedCompletionMode::ObjectMembers));
    let select_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_select_ctx,
    );
    assert_eq!(
        select_suggestions,
        vec![
            "EMP".to_string(),
            "EMP_SEQ".to_string(),
            "EMP_VIEW".to_string(),
        ]
    );
}

#[test]
fn collect_expected_keyword_suggestions_complete_common_clause_tails() {
    let cases: &[(&str, &[&str])] = &[
        ("SELECT * FROM emp ORDER |", &["BY"]),
        (
            "SELECT * FROM emp CONNECT BY PRIOR empno = mgr ORDER SIBLINGS |",
            &["BY"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST :limit |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH NEXT page_size |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 PERCENT |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST :percent PERCENT |",
            &["ROW", "ROWS"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST ROWS |",
            &["ONLY", "WITH"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS |",
            &["ONLY", "WITH"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 PERCENT ROWS |",
            &["ONLY", "WITH"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST :percent PERCENT ROWS |",
            &["ONLY", "WITH"],
        ),
        (
            "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS WITH |",
            &["TIES"],
        ),
        ("SELECT * FROM emp OFFSET 10 |", &["ROW", "ROWS"]),
        ("SELECT * FROM emp OFFSET :skip |", &["ROW", "ROWS"]),
        ("SELECT * FROM emp OFFSET 10 ROWS |", &["FETCH"]),
        ("SELECT * FROM emp OFFSET :skip ROWS |", &["FETCH"]),
    ];
    let when_ctx = analyze_inline_cursor_sql(
        "MERGE INTO target t USING src s ON (t.id = s.id) WHEN |",
    );

    for (sql, expected) in cases {
        let ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx);
        let expected: Vec<String> = expected.iter().map(|value| (*value).to_string()).collect();
        assert_eq!(suggestions, expected, "sql: {sql}");
    }

    let when_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &when_ctx);

    assert!(when_suggestions.iter().any(|value| value == "MATCHED"));
    assert!(when_suggestions.iter().any(|value| value == "NOT"));
}

#[test]
fn collect_expected_keyword_suggestions_include_ddl_object_type_tokens() {
    let create_ctx = analyze_inline_cursor_sql("CREATE |");
    let create_or_replace_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE |");
    let create_or_replace_materialized_ctx =
        analyze_inline_cursor_sql("CREATE OR REPLACE MATERIALIZED |");
    let create_or_replace_editioning_ctx =
        analyze_inline_cursor_sql("CREATE OR REPLACE EDITIONING |");
    let create_editioning_ctx = analyze_inline_cursor_sql("CREATE EDITIONING |");
    let drop_public_ctx = analyze_inline_cursor_sql("DROP PUBLIC |");
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE B|");
    let create_unique_ctx = analyze_inline_cursor_sql("CREATE UNIQUE |");
    let create_bitmap_ctx = analyze_inline_cursor_sql("CREATE BITMAP |");
    let create_global_ctx = analyze_inline_cursor_sql("CREATE GLOBAL |");
    let create_global_temporary_ctx = analyze_inline_cursor_sql("CREATE GLOBAL TEMPORARY |");
    let create_public_ctx = analyze_inline_cursor_sql("CREATE PUBLIC |");
    let create_or_replace_public_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE PUBLIC |");
    let alter_ctx = analyze_inline_cursor_sql("ALTER |");
    let alter_session_ctx = analyze_inline_cursor_sql("ALTER SESSION |");
    let alter_session_set_ctx = analyze_inline_cursor_sql("ALTER SESSION SET |");
    let analyze_ctx = analyze_inline_cursor_sql("ANALYZE |");
    let optimize_ctx = analyze_inline_cursor_sql("OPTIMIZE |");
    let check_ctx = analyze_inline_cursor_sql("CHECK |");
    let repair_ctx = analyze_inline_cursor_sql("REPAIR |");
    let create_synonym_name_ctx = analyze_inline_cursor_sql("CREATE SYNONYM emp_syn |");

    let create_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &create_ctx);
    let create_or_replace_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_ctx);
    let create_or_replace_materialized_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &create_or_replace_materialized_ctx,
        );
    let create_or_replace_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_editioning_ctx);
    let create_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_editioning_ctx);
    let drop_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_public_ctx);
    let drop_package_body_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("B", &drop_package_body_ctx);
    let create_unique_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_unique_ctx);
    let create_bitmap_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_bitmap_ctx);
    let create_global_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_global_ctx);
    let create_global_temporary_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_global_temporary_ctx);
    let create_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_public_ctx);
    let create_or_replace_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_public_ctx);
    let alter_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_ctx);
    let alter_session_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_session_ctx);
    let alter_session_set_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_session_set_ctx);
    let analyze_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &analyze_ctx);
    let optimize_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &optimize_ctx);
    let check_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &check_ctx);
    let repair_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &repair_ctx);
    let create_synonym_name_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_synonym_name_ctx);

    assert!(create_suggestions.iter().any(|value| value == "EDITIONING"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "INDEX"));
    assert!(create_or_replace_suggestions
        .iter()
        .any(|value| value == "EDITIONING"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "PACKAGE"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "TRIGGER"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "TYPE"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "USER"));
    assert_eq!(
        create_or_replace_materialized_suggestions,
        vec!["VIEW".to_string()]
    );
    assert_eq!(
        create_or_replace_editioning_suggestions,
        vec!["VIEW".to_string()]
    );
    assert_eq!(create_editioning_suggestions, vec!["VIEW".to_string()]);
    assert_eq!(drop_public_suggestions, vec!["SYNONYM".to_string()]);
    assert_eq!(drop_package_body_suggestions, vec!["BODY".to_string()]);
    assert_eq!(create_unique_suggestions, vec!["INDEX".to_string()]);
    assert_eq!(create_bitmap_suggestions, vec!["INDEX".to_string()]);
    assert_eq!(create_global_suggestions, vec!["TEMPORARY".to_string()]);
    assert_eq!(
        create_global_temporary_suggestions,
        vec!["TABLE".to_string()]
    );
    assert_eq!(create_public_suggestions, vec!["SYNONYM".to_string()]);
    assert_eq!(
        create_or_replace_public_suggestions,
        vec!["SYNONYM".to_string()]
    );
    assert!(alter_suggestions.iter().any(|value| value == "SESSION"));
    assert_eq!(alter_session_suggestions, vec!["SET".to_string()]);
    assert_eq!(
        alter_session_set_suggestions,
        vec!["CURRENT_SCHEMA".to_string()]
    );
    assert_eq!(analyze_suggestions, vec!["TABLE".to_string()]);
    assert_eq!(optimize_suggestions, vec!["TABLE".to_string()]);
    assert_eq!(check_suggestions, vec!["TABLE".to_string()]);
    assert_eq!(repair_suggestions, vec!["TABLE".to_string()]);
    assert_eq!(create_synonym_name_suggestions, vec!["FOR".to_string()]);
}

#[test]
fn collect_expected_object_suggestions_prefer_routines_for_call_context() {
    let call_ctx = analyze_inline_cursor_sql("CALL |");
    let describe_ctx = analyze_inline_cursor_sql("DESC |");
    let mut data = IntellisenseData::new();
    data.procedures = vec!["RUN_JOB".to_string()];
    data.packages = vec!["UTIL_PKG".to_string()];
    data.tables = vec!["EMP".to_string()];
    data.rebuild_indices();

    let call_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &call_ctx);
    let describe_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &describe_ctx);

    assert!(call_suggestions.iter().any(|value| value == "RUN_JOB"));
    assert!(call_suggestions.iter().any(|value| value == "UTIL_PKG"));
    assert!(!call_suggestions.iter().any(|value| value == "EMP"));
    assert!(describe_suggestions.iter().any(|value| value == "EMP"));
}

#[test]
fn collect_expected_object_suggestions_for_create_synonym_target() {
    let synonym_ctx = analyze_inline_cursor_sql("CREATE SYNONYM emp_syn FOR |");
    let public_synonym_ctx = analyze_inline_cursor_sql("CREATE PUBLIC SYNONYM emp_syn FOR |");
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.views = vec!["EMP_VIEW".to_string()];
    data.sequences = vec!["EMP_SEQ".to_string()];
    data.packages = vec!["EMP_API".to_string()];
    data.rebuild_indices();

    let synonym_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &synonym_ctx);
    let public_synonym_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &public_synonym_ctx);

    for suggestions in [&synonym_suggestions, &public_synonym_suggestions] {
        assert!(suggestions.iter().any(|value| value == "EMP"));
        assert!(suggestions.iter().any(|value| value == "EMP_VIEW"));
        assert!(suggestions.iter().any(|value| value == "EMP_SEQ"));
        assert!(suggestions.iter().any(|value| value == "EMP_API"));
    }
}

#[test]
fn schema_member_suggestions_for_create_synonym_target_include_all_object_kinds() {
    let synonym_ctx = analyze_inline_cursor_sql("CREATE SYNONYM emp_syn FOR scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![
            ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
            ("EMP_VIEW".to_string(), Some(QualifiedMemberKind::View)),
            ("EMP_SEQ".to_string(), Some(QualifiedMemberKind::Sequence)),
            ("EMP_API".to_string(), Some(QualifiedMemberKind::Package)),
            ("EMP_T".to_string(), Some(QualifiedMemberKind::Type)),
        ],
    );
    data.set_relation_members_for_qualifier(
        "SCOTT",
        vec!["EMP".to_string(), "EMP_VIEW".to_string()],
    );

    let suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &synonym_ctx,
    );

    assert_eq!(
        suggestions,
        vec![
            "EMP".to_string(),
            "EMP_API".to_string(),
            "EMP_SEQ".to_string(),
            "EMP_T".to_string(),
            "EMP_VIEW".to_string(),
        ]
    );
}

#[test]
fn collect_expected_object_suggestions_filter_by_object_type_and_include_users() {
    let drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE |");
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE BODY |");
    let drop_type_ctx = analyze_inline_cursor_sql("DROP TYPE |");
    let drop_trigger_ctx = analyze_inline_cursor_sql("DROP TRIGGER |");
    let drop_index_ctx = analyze_inline_cursor_sql("DROP INDEX |");
    let grant_execute_ctx = analyze_inline_cursor_sql("GRANT EXECUTE ON |");
    let grant_debug_ctx = analyze_inline_cursor_sql("GRANT DEBUG ON |");
    let grant_multi_relation_ctx =
        analyze_inline_cursor_sql("GRANT SELECT, INSERT, UPDATE ON |");
    let revoke_select_ctx = analyze_inline_cursor_sql("REVOKE SELECT ON |");
    let current_schema_ctx = analyze_inline_cursor_sql("ALTER SESSION SET CURRENT_SCHEMA = |");
    let prefixed_drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE sc|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.materialized_views = vec!["SALES_MV".to_string()];
    data.types = vec!["ADDRESS_T".to_string()];
    data.triggers = vec!["EMP_BIU_TRG".to_string()];
    data.indexes = vec!["EMP_PK".to_string()];
    data.procedures = vec!["RUN_JOB".to_string()];
    data.packages = vec!["UTIL_PKG".to_string()];
    data.sequences = vec!["SEQ_ORDER".to_string()];
    data.users = vec!["SCOTT".to_string()];
    data.rebuild_indices();

    let package_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_package_ctx);
    let package_body_suggestions = SqlEditorWidget::collect_expected_object_suggestions(
        &mut data,
        "",
        &drop_package_body_ctx,
    );
    let type_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_type_ctx);
    let trigger_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_trigger_ctx);
    let index_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_index_ctx);
    let grant_execute_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &grant_execute_ctx);
    let grant_debug_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &grant_debug_ctx);
    let grant_multi_relation_suggestions = SqlEditorWidget::collect_expected_object_suggestions(
        &mut data,
        "",
        &grant_multi_relation_ctx,
    );
    let revoke_select_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &revoke_select_ctx);
    let current_schema_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &current_schema_ctx);
    let prefixed_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "sc", &prefixed_drop_package_ctx);

    assert_eq!(package_suggestions, vec!["UTIL_PKG".to_string()]);
    assert_eq!(package_body_suggestions, vec!["UTIL_PKG".to_string()]);
    assert_eq!(type_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(trigger_suggestions, vec!["EMP_BIU_TRG".to_string()]);
    assert_eq!(index_suggestions, vec!["EMP_PK".to_string()]);
    assert!(grant_execute_suggestions.iter().any(|value| value == "ADDRESS_T"));
    assert!(grant_execute_suggestions.iter().any(|value| value == "RUN_JOB"));
    assert!(grant_execute_suggestions.iter().any(|value| value == "UTIL_PKG"));
    assert!(!grant_execute_suggestions.iter().any(|value| value == "EMP"));
    assert!(grant_debug_suggestions.iter().any(|value| value == "ADDRESS_T"));
    assert!(grant_debug_suggestions.iter().any(|value| value == "RUN_JOB"));
    assert!(grant_debug_suggestions.iter().any(|value| value == "UTIL_PKG"));
    assert!(!grant_debug_suggestions.iter().any(|value| value == "EMP"));
    assert!(grant_multi_relation_suggestions
        .iter()
        .any(|value| value == "EMP"));
    assert!(grant_multi_relation_suggestions
        .iter()
        .any(|value| value == "SALES_MV"));
    assert!(grant_multi_relation_suggestions
        .iter()
        .any(|value| value == "SEQ_ORDER"));
    assert!(!grant_multi_relation_suggestions
        .iter()
        .any(|value| value == "UTIL_PKG"));
    assert!(revoke_select_suggestions.iter().any(|value| value == "EMP"));
    assert!(revoke_select_suggestions
        .iter()
        .any(|value| value == "SALES_MV"));
    assert!(revoke_select_suggestions
        .iter()
        .any(|value| value == "SEQ_ORDER"));
    assert!(!revoke_select_suggestions
        .iter()
        .any(|value| value == "UTIL_PKG"));
    assert_eq!(current_schema_suggestions, vec!["SCOTT".to_string()]);
    assert_eq!(prefixed_suggestions, vec!["SCOTT".to_string()]);
}

#[test]
fn table_context_expected_object_suggestions_filter_maintenance_table_targets() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.views = vec!["EMP_VIEW".to_string()];
    data.materialized_views = vec!["EMP_MV".to_string()];
    data.packages = vec!["EMP_API".to_string()];
    data.rebuild_indices();

    for sql in [
        "ANALYZE TABLE |",
        "OPTIMIZE TABLE |",
        "CHECK TABLE |",
        "REPAIR TABLE |",
        "CREATE TABLE demo (dept_id INT, CONSTRAINT fk_demo FOREIGN KEY (dept_id) REFERENCES |)",
        "ALTER TABLE orders ADD CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES |",
        "CREATE INDEX idx_emp_dept ON |",
        "CREATE UNIQUE INDEX idx_emp_dept ON |",
        "CREATE TRIGGER trg_emp_audit ON |",
        "CREATE OR REPLACE TRIGGER trg_emp_audit BEFORE INSERT ON |",
    ] {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::table_context_expected_object_suggestions(
            &mut data, "", &deep_ctx,
        )
        .unwrap_or_else(|| panic!("expected table-target object kind for `{sql}`"));

        assert_eq!(
            suggestions,
            vec!["EMP".to_string()],
            "maintenance table target should suggest only tables for `{sql}`"
        );
    }
}

#[test]
fn expected_member_suggestions_for_qualifier_filter_schema_members_by_context() {
    let call_ctx = analyze_inline_cursor_sql("CALL scott.|");
    let drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE scott.|");
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.procedures = vec!["RUN_JOB".to_string()];
    data.packages = vec!["UTIL_PKG".to_string()];
    data.sequences = vec!["SEQ_ORDER".to_string()];
    data.rebuild_indices();
    data.set_members_for_qualifier(
        "SCOTT",
        vec![
            "EMP".to_string(),
            "RUN_JOB".to_string(),
            "UTIL_PKG".to_string(),
            "SEQ_ORDER".to_string(),
        ],
    );

    let call_suggestions =
        SqlEditorWidget::expected_member_suggestions_for_qualifier(&mut data, "scott", "", &call_ctx);
    let drop_package_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_package_ctx,
    );

    assert!(call_suggestions.iter().any(|value| value == "RUN_JOB"));
    assert!(call_suggestions.iter().any(|value| value == "UTIL_PKG"));
    assert!(!call_suggestions.iter().any(|value| value == "EMP"));
    assert!(!call_suggestions.iter().any(|value| value == "SEQ_ORDER"));
    assert_eq!(drop_package_suggestions, vec!["UTIL_PKG".to_string()]);
}

#[test]
fn expected_schema_routine_suggestions_do_not_require_top_level_type_lists() {
    let call_ctx = analyze_inline_cursor_sql("CALL scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure))],
    );

    let call_suggestions =
        SqlEditorWidget::expected_member_suggestions_for_qualifier(&mut data, "scott", "", &call_ctx);

    assert!(
        call_suggestions.iter().any(|value| value == "RUN_JOB"),
        "schema-qualified routine suggestions should not depend on current-user type caches: {:?}",
        call_suggestions
    );
}

#[test]
fn expected_schema_object_suggestions_fallback_when_member_kinds_are_unknown() {
    let call_ctx = analyze_inline_cursor_sql("CALL scott.|");
    let grant_execute_ctx = analyze_inline_cursor_sql("GRANT EXECUTE ON scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier(
        "SCOTT",
        vec!["RUN_JOB".to_string(), "UTIL_PKG".to_string()],
    );

    let call_suggestions =
        SqlEditorWidget::expected_member_suggestions_for_qualifier(&mut data, "scott", "", &call_ctx);
    let grant_execute_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_execute_ctx,
    );

    assert_eq!(
        call_suggestions,
        vec!["RUN_JOB".to_string(), "UTIL_PKG".to_string()]
    );
    assert_eq!(
        grant_execute_suggestions,
        vec!["RUN_JOB".to_string(), "UTIL_PKG".to_string()]
    );
}

#[test]
fn expected_schema_package_suggestions_do_not_require_top_level_type_lists() {
    let drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![("UTIL_PKG".to_string(), Some(QualifiedMemberKind::Package))],
    );

    let drop_package_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_package_ctx,
    );

    assert_eq!(
        drop_package_suggestions,
        vec!["UTIL_PKG".to_string()],
        "schema-qualified package suggestions should not depend on current-user package caches"
    );
}

#[test]
fn expected_package_member_routine_suggestions_do_not_require_top_level_type_lists() {
    let call_ctx = analyze_inline_cursor_sql("CALL demo_pkg.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "DEMO_PKG",
        vec![
            ("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure)),
            ("CALC_BONUS".to_string(), Some(QualifiedMemberKind::Function)),
        ],
    );

    let call_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "demo_pkg",
        "",
        &call_ctx,
    );

    assert!(call_suggestions.iter().any(|value| value == "RUN_JOB"));
    assert!(call_suggestions.iter().any(|value| value == "CALC_BONUS"));
}

#[test]
fn schema_relation_member_suggestions_filter_by_oracle_object_context() {
    let drop_table_ctx = analyze_inline_cursor_sql("DROP TABLE scott.|");
    let analyze_table_ctx = analyze_inline_cursor_sql("ANALYZE TABLE scott.|");
    let optimize_table_ctx = analyze_inline_cursor_sql("OPTIMIZE TABLE scott.|");
    let check_table_ctx = analyze_inline_cursor_sql("CHECK TABLE scott.|");
    let repair_table_ctx = analyze_inline_cursor_sql("REPAIR TABLE scott.|");
    let references_ctx = analyze_inline_cursor_sql(
        "ALTER TABLE orders ADD CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES scott.|",
    );
    let create_table_references_ctx = analyze_inline_cursor_sql(
        "CREATE TABLE demo (dept_id INT, CONSTRAINT fk_demo FOREIGN KEY (dept_id) REFERENCES scott.|)",
    );
    let create_index_ctx = analyze_inline_cursor_sql("CREATE INDEX idx_emp_dept ON scott.|");
    let create_unique_index_ctx =
        analyze_inline_cursor_sql("CREATE UNIQUE INDEX idx_emp_dept ON scott.|");
    let create_trigger_ctx = analyze_inline_cursor_sql("CREATE TRIGGER trg_emp_audit ON scott.|");
    let create_or_replace_trigger_ctx = analyze_inline_cursor_sql(
        "CREATE OR REPLACE TRIGGER trg_emp_audit BEFORE INSERT ON scott.|",
    );
    let comment_table_ctx = analyze_inline_cursor_sql("COMMENT ON TABLE scott.|");
    let comment_view_ctx = analyze_inline_cursor_sql("COMMENT ON VIEW scott.|");
    let comment_editioning_view_ctx = analyze_inline_cursor_sql("COMMENT ON EDITIONING VIEW scott.|");
    let drop_mv_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED VIEW scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![
            ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
            ("EMP_VIEW".to_string(), Some(QualifiedMemberKind::View)),
            (
                "EMP_MV".to_string(),
                Some(QualifiedMemberKind::MaterializedView),
            ),
            ("UTIL_PKG".to_string(), Some(QualifiedMemberKind::Package)),
        ],
    );
    data.set_relation_members_for_qualifier(
        "SCOTT",
        vec![
            "EMP".to_string(),
            "EMP_VIEW".to_string(),
            "EMP_MV".to_string(),
        ],
    );

    let table_suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_table_ctx,
    );
    let analyze_table_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &analyze_table_ctx,
        );
    let optimize_table_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &optimize_table_ctx,
        );
    let check_table_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &check_table_ctx,
        );
    let repair_table_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &repair_table_ctx,
        );
    let references_suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &references_ctx,
    );
    let create_table_references_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_table_references_ctx,
        );
    let create_index_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_index_ctx,
        );
    let create_unique_index_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_unique_index_ctx,
        );
    let create_trigger_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_trigger_ctx,
        );
    let create_or_replace_trigger_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_or_replace_trigger_ctx,
        );
    let mv_suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_mv_ctx,
    );
    let comment_table_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &comment_table_ctx,
        );
    let comment_view_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &comment_view_ctx,
        );
    let comment_editioning_view_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &comment_editioning_view_ctx,
    );

    assert_eq!(table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(analyze_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(optimize_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(check_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(repair_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(references_suggestions, vec!["EMP".to_string()]);
    assert_eq!(create_table_references_suggestions, vec!["EMP".to_string()]);
    assert_eq!(create_index_suggestions, vec!["EMP".to_string()]);
    assert_eq!(create_unique_index_suggestions, vec!["EMP".to_string()]);
    assert_eq!(create_trigger_suggestions, vec!["EMP".to_string()]);
    assert_eq!(
        create_or_replace_trigger_suggestions,
        vec!["EMP".to_string()]
    );
    assert_eq!(mv_suggestions, vec!["EMP_MV".to_string()]);
    assert_eq!(comment_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(comment_view_suggestions, vec!["EMP_VIEW".to_string()]);
    assert_eq!(comment_editioning_view_suggestions, vec!["EMP_VIEW".to_string()]);
}

#[test]
fn schema_object_member_suggestions_cover_oracle_ddl_object_types() {
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE BODY scott.|");
    let drop_type_ctx = analyze_inline_cursor_sql("DROP TYPE scott.|");
    let drop_type_body_ctx = analyze_inline_cursor_sql("DROP TYPE BODY scott.|");
    let alter_trigger_ctx = analyze_inline_cursor_sql("ALTER TRIGGER scott.|");
    let drop_index_ctx = analyze_inline_cursor_sql("DROP INDEX scott.|");
    let grant_execute_ctx = analyze_inline_cursor_sql("GRANT EXECUTE ON scott.|");
    let grant_debug_ctx = analyze_inline_cursor_sql("GRANT DEBUG ON scott.|");
    let grant_select_ctx = analyze_inline_cursor_sql("GRANT SELECT ON scott.|");
    let grant_multi_relation_ctx =
        analyze_inline_cursor_sql("GRANT SELECT, INSERT, UPDATE ON scott.|");
    let mut data = IntellisenseData::new();
    data.set_members_for_qualifier_with_kinds(
        "SCOTT",
        vec![
            ("EMP".to_string(), Some(QualifiedMemberKind::Table)),
            (
                "SALES_MV".to_string(),
                Some(QualifiedMemberKind::MaterializedView),
            ),
            ("SEQ_ORDER".to_string(), Some(QualifiedMemberKind::Sequence)),
            ("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure)),
            ("UTIL_PKG".to_string(), Some(QualifiedMemberKind::Package)),
            ("ADDRESS_T".to_string(), Some(QualifiedMemberKind::Type)),
            ("EMP_BIU_TRG".to_string(), Some(QualifiedMemberKind::Trigger)),
            ("EMP_PK".to_string(), Some(QualifiedMemberKind::Index)),
        ],
    );

    let package_body_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_package_body_ctx,
    );
    let type_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_type_ctx,
    );
    let type_body_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_type_body_ctx,
    );
    let trigger_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &alter_trigger_ctx,
    );
    let index_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_index_ctx,
    );
    let grant_execute_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_execute_ctx,
    );
    let grant_debug_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_debug_ctx,
    );
    let grant_select_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &grant_select_ctx,
    );
    let grant_multi_relation_suggestions =
        SqlEditorWidget::expected_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &grant_multi_relation_ctx,
        );

    assert_eq!(package_body_suggestions, vec!["UTIL_PKG".to_string()]);
    assert_eq!(type_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(type_body_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(trigger_suggestions, vec!["EMP_BIU_TRG".to_string()]);
    assert_eq!(index_suggestions, vec!["EMP_PK".to_string()]);
    assert_eq!(
        grant_execute_suggestions,
        vec![
            "ADDRESS_T".to_string(),
            "RUN_JOB".to_string(),
            "UTIL_PKG".to_string()
        ]
    );
    assert_eq!(
        grant_debug_suggestions,
        vec![
            "ADDRESS_T".to_string(),
            "RUN_JOB".to_string(),
            "UTIL_PKG".to_string()
        ]
    );
    assert_eq!(
        grant_select_suggestions,
        vec![
            "EMP".to_string(),
            "SALES_MV".to_string(),
            "SEQ_ORDER".to_string()
        ]
    );
    assert_eq!(
        grant_multi_relation_suggestions,
        vec![
            "EMP".to_string(),
            "SALES_MV".to_string(),
            "SEQ_ORDER".to_string()
        ]
    );
}

#[test]
fn collect_expected_keyword_suggestions_complete_materialized_view_tail() {
    let drop_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED |");
    let comment_on_ctx = analyze_inline_cursor_sql("COMMENT ON |");
    let comment_editioning_ctx = analyze_inline_cursor_sql("COMMENT ON EDITIONING |");
    let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_ctx);
    let comment_on_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &comment_on_ctx);
    let comment_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &comment_editioning_ctx);

    assert_eq!(suggestions, vec!["VIEW".to_string()]);
    assert_eq!(comment_editioning_suggestions, vec!["VIEW".to_string()]);
    assert!(comment_on_suggestions.iter().any(|value| value == "COLUMN"));
    assert!(comment_on_suggestions
        .iter()
        .any(|value| value == "EDITIONING"));
    assert!(comment_on_suggestions
        .iter()
        .any(|value| value == "MATERIALIZED"));
}

#[test]
fn finalize_completion_after_selection_clears_pending_and_invalidates_generation() {
    let runtime = runtime_state_for_test(
        Some((5, 10)),
        Some(PendingIntellisense { cursor_pos: 10 }),
        3,
        9,
    );

    SqlEditorWidget::finalize_completion_after_selection(&runtime);

    assert!(runtime.completion_range().is_none());
    assert!(runtime.pending_intellisense().is_none());
    assert_eq!(runtime.current_keyup_generation(), 4);
    assert_eq!(runtime.current_parse_generation(), 10);
}

#[test]
fn completion_insert_text_keeps_existing_left_qualifier_for_condition_comparison() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text("a.abc = b.abc"),
        "abc = b.abc"
    );
}

#[test]
fn completion_caret_offset_lands_between_function_parentheses() {
    // Function completions end with "()"; caret goes between the parens.
    assert_eq!(SqlEditorWidget::completion_caret_offset("NVL()"), 4);
    assert_eq!(SqlEditorWidget::completion_caret_offset("COALESCE()"), 9);
}

#[test]
fn completion_caret_offset_lands_at_end_for_plain_identifiers() {
    assert_eq!(
        SqlEditorWidget::completion_caret_offset("employee_id"),
        "employee_id".len()
    );
    assert_eq!(
        SqlEditorWidget::completion_caret_offset("abc = b.abc"),
        "abc = b.abc".len()
    );
}

#[test]
fn completion_insert_text_handles_quoted_multi_part_left_qualifier() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text(
            "\"sales\".\"Order Header\".\"Order Id\" = b.\"Order Id\""
        ),
        "\"Order Id\" = b.\"Order Id\""
    );
}

#[test]
fn completion_insert_text_ignores_equals_inside_quoted_column_name() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text(r#"a."A = B" = b."A = B""#),
        r#""A = B" = b."A = B""#
    );
}

#[test]
fn completion_insert_text_handles_backtick_quoted_condition_comparison() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text("a.`A = B` = b.`A = B`"),
        "`A = B` = b.`A = B`"
    );
}

#[test]
fn completion_insert_text_does_not_treat_named_argument_arrow_as_condition_comparison() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text("pkg.proc(arg => value)"),
        "pkg.proc(arg => value)"
    );
}

#[test]
fn completion_insert_text_does_not_treat_unspaced_equals_as_condition_comparison() {
    assert_eq!(
        SqlEditorWidget::completion_insert_text("a.abc=b.abc"),
        "a.abc=b.abc"
    );
}

#[test]
fn auto_join_condition_prefix_matches_left_column_identifier() {
    assert!(SqlEditorWidget::completion_suggestion_matches_prefix(
        r#"e."Dept No" = d."Dept No""#,
        "Dept"
    ));
    assert!(SqlEditorWidget::completion_suggestion_matches_prefix(
        "e.[Item]]Id] = d.[Item]]Id]",
        "Item]I"
    ));
    assert!(SqlEditorWidget::completion_suggestion_matches_prefix(
        r#"e."Dept No" = d."Dept No""#,
        "e."
    ));
    assert!(!SqlEditorWidget::completion_suggestion_matches_prefix(
        r#"e."Dept No" = d."Dept No""#,
        "Missing"
    ));
}

#[test]
fn completion_replacement_range_extends_zero_length_range_over_forward_identifier() {
    let sql_with_cursor = "SELECT * FROM tb1 a JOIN tb2 b ON a.|a";
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let (word, word_start, word_end) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);

    let range = SqlEditorWidget::completion_replacement_range_from_word_bounds(
        &word,
        word_start,
        word_end,
        cursor,
        Some((cursor, cursor)),
    );

    assert_eq!(range, (cursor, cursor + 1));
}

#[test]
fn build_column_descriptions_formats_type_pk_nn_and_fk() {
    use crate::ui::intellisense::{ColumnMeta, ForeignKeyMeta};

    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMP",
        vec![
            "EMP_ID".to_string(),
            "ENAME".to_string(),
            "DEPTNO".to_string(),
        ],
    );

    let mut meta = HashMap::new();
    meta.insert(
        "EMP_ID".to_string(),
        ColumnMeta {
            type_display: "NUMBER(10)".to_string(),
            nullable: false,
            is_primary_key: true,
        },
    );
    meta.insert(
        "ENAME".to_string(),
        ColumnMeta {
            type_display: "VARCHAR2(50)".to_string(),
            nullable: true,
            is_primary_key: false,
        },
    );
    meta.insert(
        "DEPTNO".to_string(),
        ColumnMeta {
            type_display: "NUMBER".to_string(),
            nullable: false,
            is_primary_key: false,
        },
    );
    data.set_column_meta_for_table("EMP", meta);
    data.set_foreign_keys_for_table(
        "EMP",
        vec![ForeignKeyMeta {
            columns: vec!["DEPTNO".to_string()],
            ref_table: "DEPT".to_string(),
            ref_columns: vec!["DEPTNO".to_string()],
        }],
    );

    let descriptions =
        SqlEditorWidget::build_column_descriptions(&data, &["EMP".to_string()]);

    assert_eq!(descriptions.get("EMP_ID").map(String::as_str), Some("NUMBER(10)  PK"));
    assert_eq!(descriptions.get("ENAME").map(String::as_str), Some("VARCHAR2(50)"));
    // Non-PK NOT NULL column that is also a foreign key shows both badges.
    assert_eq!(
        descriptions.get("DEPTNO").map(String::as_str),
        Some("NUMBER  NN  FK\u{2192}DEPT")
    );
}

#[test]
fn build_column_descriptions_match_quoted_column_metadata_and_fk() {
    use crate::ui::intellisense::{ColumnMeta, ForeignKeyMeta};

    let mut data = IntellisenseData::new();
    data.set_columns_for_table(
        "EMP",
        vec![r#""Dept No""#.to_string(), r#""Emp Name""#.to_string()],
    );

    let mut meta = HashMap::new();
    meta.insert(
        r#""Dept No""#.to_string(),
        ColumnMeta {
            type_display: "NUMBER".to_string(),
            nullable: false,
            is_primary_key: false,
        },
    );
    meta.insert(
        r#""Emp Name""#.to_string(),
        ColumnMeta {
            type_display: "VARCHAR2(50)".to_string(),
            nullable: true,
            is_primary_key: false,
        },
    );
    data.set_column_meta_for_table("EMP", meta);
    data.set_foreign_keys_for_table(
        "EMP",
        vec![ForeignKeyMeta {
            columns: vec![r#""Dept No""#.to_string()],
            ref_table: "DEPT".to_string(),
            ref_columns: vec![r#""Dept No""#.to_string()],
        }],
    );

    let descriptions =
        SqlEditorWidget::build_column_descriptions(&data, &["EMP".to_string()]);

    assert_eq!(
        descriptions.get("DEPT NO").map(String::as_str),
        Some("NUMBER  NN  FK\u{2192}DEPT")
    );
    assert_eq!(
        descriptions.get("EMP NAME").map(String::as_str),
        Some("VARCHAR2(50)")
    );
}

#[test]
fn build_auto_join_condition_uses_fk_in_either_direction() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    // EMP.DEPTNO references DEPT.DEPTNO.
    data.set_foreign_keys_for_table(
        "EMP",
        vec![ForeignKeyMeta {
            columns: vec!["DEPTNO".to_string()],
            ref_table: "DEPT".to_string(),
            ref_columns: vec!["DEPTNO".to_string()],
        }],
    );

    let emp = ScopedTableRef {
        name: "EMP".to_string(),
        alias: Some("e".to_string()),
        depth: 0,
        is_cte: false,
    };
    let dept = ScopedTableRef {
        name: "DEPT".to_string(),
        alias: Some("d".to_string()),
        depth: 0,
        is_cte: false,
    };

    // FROM dept JOIN emp ON | -> right=emp owns the FK toward dept.
    assert_eq!(
        SqlEditorWidget::build_auto_join_condition(&data, &emp, &[&dept]),
        Some("e.DEPTNO = d.DEPTNO".to_string())
    );

    // FROM emp JOIN dept ON | -> left=emp owns the FK toward right=dept.
    assert_eq!(
        SqlEditorWidget::build_auto_join_condition(&data, &dept, &[&emp]),
        Some("e.DEPTNO = d.DEPTNO".to_string())
    );

    // No FK relationship -> no suggestion.
    let bonus = ScopedTableRef {
        name: "BONUS".to_string(),
        alias: Some("b".to_string()),
        depth: 0,
        is_cte: false,
    };
    assert_eq!(
        SqlEditorWidget::build_auto_join_condition(&data, &bonus, &[&dept]),
        None
    );
}

#[test]
fn build_auto_join_condition_quotes_columns_that_need_identifier_quotes() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    data.set_foreign_keys_for_table(
        "EMP",
        vec![ForeignKeyMeta {
            columns: vec!["Dept No".to_string()],
            ref_table: "DEPT".to_string(),
            ref_columns: vec!["Dept No".to_string()],
        }],
    );

    let emp = ScopedTableRef {
        name: "EMP".to_string(),
        alias: Some("e".to_string()),
        depth: 0,
        is_cte: false,
    };
    let dept = ScopedTableRef {
        name: "DEPT".to_string(),
        alias: Some("Dept Alias".to_string()),
        depth: 0,
        is_cte: false,
    };

    let condition = SqlEditorWidget::build_auto_join_condition(&data, &dept, &[&emp]);

    assert_eq!(
        condition.as_deref(),
        Some(r#"e."Dept No" = "Dept Alias"."Dept No""#)
    );
}

#[test]
fn build_auto_join_condition_does_not_match_quoted_dotted_table_by_short_suffix() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    data.set_foreign_keys_for_table(
        "ORDERS",
        vec![ForeignKeyMeta {
            columns: vec!["DAILY_ID".to_string()],
            ref_table: r#""sales.daily""#.to_string(),
            ref_columns: vec!["ID".to_string()],
        }],
    );

    let orders = ScopedTableRef {
        name: "ORDERS".to_string(),
        alias: Some("o".to_string()),
        depth: 0,
        is_cte: false,
    };
    let daily = ScopedTableRef {
        name: "DAILY".to_string(),
        alias: Some("d".to_string()),
        depth: 0,
        is_cte: false,
    };

    let condition = SqlEditorWidget::build_auto_join_condition(&data, &orders, &[&daily]);

    assert_eq!(condition, None);
}

#[test]
fn build_auto_join_condition_does_not_match_bracket_dotted_table_by_short_suffix() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    data.set_foreign_keys_for_table(
        "ORDERS",
        vec![ForeignKeyMeta {
            columns: vec!["DAILY_ID".to_string()],
            ref_table: "[sales.daily]".to_string(),
            ref_columns: vec!["ID".to_string()],
        }],
    );

    let orders = ScopedTableRef {
        name: "ORDERS".to_string(),
        alias: Some("o".to_string()),
        depth: 0,
        is_cte: false,
    };
    let daily = ScopedTableRef {
        name: "DAILY".to_string(),
        alias: Some("d".to_string()),
        depth: 0,
        is_cte: false,
    };

    let condition = SqlEditorWidget::build_auto_join_condition(&data, &orders, &[&daily]);

    assert_eq!(condition, None);
}

#[test]
fn build_auto_join_condition_preserves_unaliased_quoted_dotted_table_qualifier() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    data.set_foreign_keys_for_table(
        "ORDERS",
        vec![ForeignKeyMeta {
            columns: vec!["DAILY_ID".to_string()],
            ref_table: r#""sales.daily""#.to_string(),
            ref_columns: vec!["ID".to_string()],
        }],
    );

    let orders = ScopedTableRef {
        name: "ORDERS".to_string(),
        alias: Some("o".to_string()),
        depth: 0,
        is_cte: false,
    };
    let quoted_daily = ScopedTableRef {
        name: r#""sales.daily""#.to_string(),
        alias: None,
        depth: 0,
        is_cte: false,
    };

    let condition = SqlEditorWidget::build_auto_join_condition(&data, &orders, &[&quoted_daily]);

    assert_eq!(
        condition.as_deref(),
        Some(r#"o.DAILY_ID = "sales.daily".ID"#)
    );
}

#[test]
fn build_auto_join_condition_preserves_unaliased_bracket_dotted_table_qualifier() {
    use crate::ui::intellisense::ForeignKeyMeta;
    use crate::ui::intellisense_context::ScopedTableRef;

    let mut data = IntellisenseData::new();
    data.set_foreign_keys_for_table(
        "ORDERS",
        vec![ForeignKeyMeta {
            columns: vec!["DAILY_ID".to_string()],
            ref_table: "[sales.daily]".to_string(),
            ref_columns: vec!["ID".to_string()],
        }],
    );

    let orders = ScopedTableRef {
        name: "ORDERS".to_string(),
        alias: Some("o".to_string()),
        depth: 0,
        is_cte: false,
    };
    let bracket_daily = ScopedTableRef {
        name: "[sales.daily]".to_string(),
        alias: None,
        depth: 0,
        is_cte: false,
    };

    let condition = SqlEditorWidget::build_auto_join_condition(&data, &orders, &[&bracket_daily]);

    assert_eq!(
        condition.as_deref(),
        Some(r#"o.DAILY_ID = "sales.daily".ID"#)
    );
}

#[test]
fn build_signature_label_formats_params_and_spans() {
    fn proc_arg(
        name: Option<&str>,
        position: i32,
        in_out: &str,
        data_type: Option<&str>,
        data_length: Option<i32>,
    ) -> crate::db::ProcedureArgument {
        crate::db::ProcedureArgument {
            name: name.map(str::to_string),
            position,
            sequence: position,
            data_type: data_type.map(str::to_string),
            in_out: Some(in_out.to_string()),
            data_length,
            data_precision: None,
            data_scale: None,
            type_owner: None,
            type_name: None,
            pls_type: None,
            overload: None,
            default_value: None,
        }
    }

    let args = vec![
        proc_arg(None, 0, "OUT", Some("NUMBER"), None), // return type
        proc_arg(Some("P_ID"), 1, "IN", Some("NUMBER"), None),
        proc_arg(Some("P_NAME"), 2, "IN", Some("VARCHAR2"), Some(50)),
    ];

    let label = SqlEditorWidget::build_signature_label("myfunc", &args);

    assert_eq!(
        label.text,
        "MYFUNC(P_ID IN NUMBER, P_NAME IN VARCHAR2(50)) RETURN NUMBER"
    );
    assert_eq!(label.arg_spans.len(), 2);
    let (s0, e0) = label.arg_spans[0];
    let (s1, e1) = label.arg_spans[1];
    assert_eq!(&label.text[s0..e0], "P_ID IN NUMBER");
    assert_eq!(&label.text[s1..e1], "P_NAME IN VARCHAR2(50)");
}
