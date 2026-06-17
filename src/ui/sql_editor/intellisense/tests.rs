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

/// JOIN-clause completion precision: the join target is a table position, the
/// slot after a complete target is `ON`/`USING` only (relations suppressed), and
/// an `ON` condition resolves to the columns of every joined table — qualified
/// to a single alias when one is given. Guards against the common regressions of
/// leaking relations into `ON`, or losing one side of the join.
#[test]
fn join_clause_completion_is_scoped_to_joined_tables() {
    let coltabs = |sql: &str| {
        let ctx = analyze_inline_cursor_sql(sql);
        let mut tabs = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        tabs.sort();
        (ctx.phase, tabs)
    };
    use intellisense_context::SqlPhase;

    // Join target is a table position (FromClause), not a column position.
    for sql in [
        "SELECT * FROM a JOIN |",
        "SELECT * FROM a LEFT JOIN |",
        "SELECT * FROM a NATURAL JOIN |",
        "SELECT * FROM a CROSS JOIN |",
    ] {
        assert_eq!(coltabs(sql).0, SqlPhase::FromClause, "{sql}");
    }

    // After a complete join target only `ON`/`USING` are grammatical, so the
    // identifier list is suppressed and those keywords are offered.
    for sql in [
        "SELECT * FROM a JOIN b |",
        "SELECT * FROM a INNER JOIN b |",
        "SELECT * FROM a JOIN b AS x |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_after_complete_join_target_for_context(&ctx, false),
            "complete target for `{sql}`"
        );
        let kw = SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &ctx,
            Some(crate::db::DatabaseType::Oracle),
        );
        assert!(kw.iter().any(|k| k == "ON"), "ON for `{sql}`");
        assert!(kw.iter().any(|k| k == "USING"), "USING for `{sql}`");
    }

    // An `ON` condition (and its `AND`/value continuations) sees every joined
    // table — both sides, and all three in a chained join.
    assert_eq!(
        coltabs("SELECT * FROM emp e JOIN dept d ON |"),
        (SqlPhase::JoinCondition, vec!["dept".to_string(), "emp".to_string()])
    );
    assert_eq!(
        coltabs("SELECT * FROM a JOIN b ON a.id = |").1,
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        coltabs("SELECT * FROM a JOIN b ON a.x=b.x JOIN c ON |").1,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    // Old-style comma join condition sees both relations too.
    assert_eq!(
        coltabs("SELECT * FROM a, b WHERE a.id = |").1,
        vec!["a".to_string(), "b".to_string()]
    );

    // A qualified reference inside `ON` resolves to exactly that alias's table.
    let qualified = |sql: &str, q: &str| {
        SqlEditorWidget::resolve_column_tables_for_context(
            Some(q),
            &analyze_inline_cursor_sql(sql),
        )
    };
    assert_eq!(
        qualified("SELECT * FROM emp e JOIN dept d ON e.|", "e"),
        vec!["emp".to_string()]
    );
    assert_eq!(
        qualified("SELECT * FROM emp e JOIN dept d ON e.id = d.|", "d"),
        vec!["dept".to_string()]
    );
    // USING resolves against both relations (common-column intersection).
    assert_eq!(
        coltabs("SELECT * FROM a JOIN b USING (|)").0,
        SqlPhase::JoinUsingColumnList
    );
}

/// A PL/SQL `%`-attribute slot (`<var>%|`, `<table>%|`, `<table>.<col>%|`)
/// accepts only `TYPE`/`ROWTYPE` (a column has no `ROWTYPE`), so every other
/// identifier source is suppressed and the attributes are offered. The modulo
/// operator in an expression must stay a value position.
#[test]
fn plsql_type_attribute_slot_offers_type_rowtype_and_suppresses_identifiers() {
    let kw = |sql: &str, prefix: &str, excl: bool| {
        let ctx = analyze_inline_cursor_sql(sql);
        (
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, excl),
            SqlEditorWidget::collect_expected_keyword_suggestions(
                prefix,
                &ctx,
                Some(crate::db::DatabaseType::Oracle),
            ),
        )
    };
    let (s, k) = kw("DECLARE v emp.sal%| BEGIN NULL; END;", "", false);
    assert!(s);
    assert_eq!(k, vec!["TYPE".to_string()]);
    let (s, k) = kw("DECLARE v emp%| BEGIN NULL; END;", "", false);
    assert!(s);
    assert_eq!(k, vec!["TYPE".to_string(), "ROWTYPE".to_string()]);
    // Mid-typed (`%t|`): production excludes the prefix, still recognised.
    let (s, k) = kw("DECLARE v emp.sal%t| BEGIN NULL; END;", "t", true);
    assert!(s);
    assert_eq!(k, vec!["TYPE".to_string()]);
    // Modulo is never a `%`-attribute: it stays a value position.
    assert!(!kw("DECLARE v NUMBER := a % | BEGIN NULL; END;", "", false).0);
    assert!(!kw("SELECT a % | FROM t", "", false).0);
}

/// The `GRANT |` / `REVOKE |` privilege list offers privilege keywords, but
/// because a role name is also grantable there the slot is *not* suppressed —
/// privileges merge alongside the identifier base. The grantee slot has none.
#[test]
fn grant_privilege_list_offers_privileges_without_suppressing_roles() {
    let kw = |sql: &str| {
        let ctx = analyze_inline_cursor_sql(sql);
        (
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            SqlEditorWidget::collect_expected_keyword_suggestions(
                "",
                &ctx,
                Some(crate::db::DatabaseType::Oracle),
            ),
        )
    };
    for sql in [
        "GRANT | ON t TO u",
        "GRANT SELECT, | ON t TO u",
        "REVOKE | ON t FROM u",
        "GRANT | TO u",
    ] {
        let (suppress, k) = kw(sql);
        assert!(!suppress, "must not suppress (roles valid) for `{sql}`");
        assert!(k.iter().any(|p| p == "SELECT"), "SELECT for `{sql}`");
        assert!(k.iter().any(|p| p == "EXECUTE"), "EXECUTE for `{sql}`");
    }
    // The grantee slot is not a privilege position.
    assert!(!kw("GRANT SELECT ON t TO |").1.iter().any(|p| p == "SELECT"));
}

/// The brand-new object name of a `CREATE` statement (`CREATE TABLE |`,
/// `CREATE OR REPLACE PACKAGE |`, `CREATE MATERIALIZED VIEW |`, …) never
/// references an existing object, so the relation/object catalog is suppressed.
/// `DROP`/`ALTER <type> <name>` name an existing object and are untouched, as are
/// the object-type keyword slots themselves (`CREATE |`, `CREATE MATERIALIZED |`).
#[test]
fn create_object_new_name_slot_suppresses_existing_objects() {
    let is_new_name = |sql: &str| {
        SqlEditorWidget::cursor_is_at_create_object_new_name(
            &analyze_inline_cursor_sql(sql),
            true,
        )
    };
    for sql in [
        "CREATE TABLE my|",
        "CREATE OR REPLACE PACKAGE p|",
        "CREATE OR REPLACE PACKAGE BODY p|",
        "CREATE MATERIALIZED VIEW m|",
        "CREATE UNIQUE INDEX i|",
        "CREATE GLOBAL TEMPORARY TABLE t|",
        "CREATE TABLE IF NOT EXISTS t|",
    ] {
        assert!(is_new_name(sql), "new name for `{sql}`");
    }
    for sql in [
        "CREATE |",                 // object-type keyword slot
        "CREATE MATERIALIZED |",    // -> VIEW keyword
        "CREATE INDEX i ON t|",     // existing table target
        "DROP TABLE t|",            // existing object
        "ALTER TABLE t|",           // existing object
        "SELECT | FROM t",          // unrelated
    ] {
        assert!(!is_new_name(sql), "must not flag `{sql}`");
    }
}

/// The start of a window specification (`OVER (|)` / `WINDOW name AS (|)`)
/// accepts only the clause openers (`PARTITION BY`/`ORDER BY`/frame units) or a
/// window-name reference, never a bare column. The column list the surrounding
/// expression phase would offer is suppressed and the openers are emitted; once
/// the clause body begins (`PARTITION BY |`, `ORDER BY |`) columns return.
#[test]
fn window_spec_start_suppresses_columns_and_offers_clause_openers() {
    let openers = |sql: &str, prefix: &str, exclude: bool| {
        let ctx = analyze_inline_cursor_sql(sql);
        (
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, exclude),
            SqlEditorWidget::collect_expected_keyword_suggestions(
                prefix,
                &ctx,
                Some(crate::db::DatabaseType::Oracle),
            ),
        )
    };
    for sql in [
        "SELECT sum(x) OVER (|) FROM t",
        "SELECT count(*) FROM t WINDOW w AS (|)",
    ] {
        let (suppress, kw) = openers(sql, "", false);
        assert!(suppress, "suppress for `{sql}`");
        assert!(kw.iter().any(|k| k == "PARTITION BY"), "PARTITION BY for `{sql}`");
        assert!(kw.iter().any(|k| k == "ORDER BY"), "ORDER BY for `{sql}`");
        assert!(kw.iter().any(|k| k == "ROWS"), "ROWS for `{sql}`");
    }
    // Mid-typed opener (`OVER (PART|)`): the production path excludes the typed
    // prefix, so it is still recognised and narrows to `PARTITION BY`.
    let (suppress, kw) = openers("SELECT sum(x) OVER (PART|) FROM t", "PART", true);
    assert!(suppress);
    assert_eq!(kw, vec!["PARTITION BY".to_string()]);

    // Inside the clause body columns must remain available.
    for sql in [
        "SELECT sum(x) OVER (PARTITION BY |) FROM t",
        "SELECT sum(x) OVER (ORDER BY |) FROM t",
        "SELECT sum(x) OVER (PARTITION BY a, |) FROM t",
        // A non-window parenthesised expression is never a window-spec start.
        "SELECT (|) FROM t",
        "SELECT coalesce(|) FROM t",
    ] {
        assert!(
            !openers(sql, "", false).0,
            "must not suppress for `{sql}`"
        );
    }
}

/// An Oracle `TABLESAMPLE` value slot (`FROM t SAMPLE (|)`, `SAMPLE BLOCK (|)`,
/// `... SEED (|)`) accepts only a numeric sampling percentage / seed, never a
/// relation, even though the cursor is still in the `FROM` table phase.
#[test]
fn table_sample_value_slot_suppresses_relations() {
    for sql in [
        "SELECT * FROM t SAMPLE (|)",
        "SELECT * FROM t SAMPLE BLOCK (|)",
        "SELECT * FROM t SAMPLE (10) SEED (|)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "suppress for `{sql}`"
        );
    }
    // An ordinary FROM relation position is untouched.
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("SELECT * FROM |"),
        false,
    ));
}

/// A foreign-key referential action slot (`... REFERENCES t (...) ON DELETE |`
/// / `ON UPDATE |`) accepts only a fixed action keyword, never a relation. The
/// `DELETE`/`UPDATE` keyword there must not be mistaken for a DML statement (it
/// previously fell through to the `DeleteTarget`/`UpdateTarget` table phase and
/// offered the entire table catalog).
#[test]
fn referential_action_slot_suppresses_tables_and_offers_action_keywords() {
    for sql in [
        "CREATE TABLE c (id NUMBER REFERENCES p (id) ON DELETE |)",
        "CREATE TABLE c (id NUMBER REFERENCES p (id) ON UPDATE |)",
        "ALTER TABLE c ADD CONSTRAINT fk FOREIGN KEY (pid) REFERENCES p (id) ON DELETE |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        // Not a table-target phase any more.
        assert!(!ctx.phase.is_table_context(), "phase for `{sql}`");
        // Routes through the single column/identifier-suppression chokepoint.
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "suppression for `{sql}`"
        );
    }

    let actions = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    let on_delete = actions("CREATE TABLE c (id NUMBER REFERENCES p (id) ON DELETE |)");
    assert!(on_delete.iter().any(|k| k == "CASCADE"));
    assert!(on_delete.iter().any(|k| k == "SET NULL"));
    // The action slot does not leak the standalone-`DELETE` continuation `FROM`.
    assert!(!on_delete.iter().any(|k| k == "FROM"));
    let on_update = actions("CREATE TABLE c (id NUMBER REFERENCES p (id) ON UPDATE |)");
    assert!(on_update.iter().any(|k| k == "CASCADE"));
    assert!(on_update.iter().any(|k| k == "CURRENT_TIMESTAMP"));

    // A standalone DML `DELETE`/`UPDATE` (not preceded by `ON`) is untouched.
    assert!(actions("DELETE |").iter().any(|k| k == "FROM"));
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("DELETE |"),
        false,
    ));
}

/// ORDER BY sort modifier tails accept only the next fixed modifier keyword:
/// `<sort-key> ASC|DESC |` -> `NULLS`,
/// `<sort-key> [ASC|DESC] NULLS |` -> `FIRST`/`LAST`, and the completed
/// `NULLS FIRST|LAST |` tail accepts no identifier until a comma starts the next
/// key. The ORDER BY column list and the flat operand-start keyword dump are
/// both suppressed there.
#[test]
fn order_by_sort_modifier_slots_suppress_columns_and_offer_only_modifier_keywords() {
    let kw = |sql: &str| {
        let cursor = sql.find('|').expect("cursor marker");
        let s = sql.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql);
        let prefix = crate::ui::intellisense::get_word_at_cursor(&s, cursor).0;
        SqlEditorWidget::collect_expected_keyword_suggestions(
            &prefix,
            &ctx,
            Some(crate::db::DatabaseType::Oracle),
        )
    };

    for sql in [
        "SELECT * FROM t ORDER BY id ASC |",
        "SELECT * FROM t ORDER BY id DESC |",
        "SELECT * FROM t ORDER BY id ASC N|",
        "SELECT a, b FROM t ORDER BY a ASC, b DESC |",
    ] {
        let cursor = sql.find('|').expect("cursor marker");
        let s = sql.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql);
        let has_prefix = !crate::ui::intellisense::get_word_at_cursor(&s, cursor).0.is_empty();
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, has_prefix),
            "suppression for `{sql}`"
        );
        assert_eq!(kw(sql), vec!["NULLS".to_string()], "for `{sql}`");
    }

    for sql in [
        "SELECT * FROM t ORDER BY id NULLS |",
        "SELECT * FROM t ORDER BY id ASC NULLS |",
        "SELECT * FROM t ORDER BY id DESC NULLS |",
    ] {
        let cursor = sql.find('|').expect("cursor marker");
        let s = sql.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql);
        let has_prefix = !crate::ui::intellisense::get_word_at_cursor(&s, cursor).0.is_empty();
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, has_prefix),
            "suppression for `{sql}`"
        );
        let suggestions = kw(sql);
        assert!(suggestions.iter().any(|k| k == "FIRST"), "FIRST for `{sql}`");
        assert!(suggestions.iter().any(|k| k == "LAST"), "LAST for `{sql}`");
    }
    assert_eq!(
        kw("SELECT * FROM t ORDER BY id NULLS F|"),
        vec!["FIRST".to_string()]
    );
    assert_eq!(
        kw("SELECT * FROM t ORDER BY id DESC NULLS L|"),
        vec!["LAST".to_string()]
    );
    for sql in [
        "SELECT * FROM t ORDER BY id NULLS FIRST |",
        "SELECT * FROM t ORDER BY id ASC NULLS LAST |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "completed modifier tail should suppress for `{sql}`"
        );
        assert!(kw(sql).is_empty(), "no keyword should follow `{sql}`");
    }

    // Lookalikes outside an ORDER BY sort modifier tail are untouched.
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("SELECT t.nulls | FROM t"),
        false,
    ));
    assert!(kw("SELECT nulls | FROM t").is_empty());
    assert!(kw("CREATE INDEX ix ON t (id ASC |").is_empty());
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("CREATE INDEX ix ON t (id ASC |"),
        false,
    ));
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("SELECT * FROM t ORDER BY id A|"),
        true,
    ));
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("SELECT * FROM t ORDER BY first |"),
        false,
    ));
}

/// The definition-list classifier keys off the `TABLE` keyword, so it must not
/// engage for statements that merely contain `TABLE` without being a
/// `CREATE TABLE` / `ALTER TABLE` definition (`CREATE TYPE … AS TABLE OF …`),
/// nor for `ALTER` of a non-table object, nor leak across a statement boundary.
#[test]
fn ddl_definition_list_detector_does_not_misfire_on_lookalikes() {
    // `TABLE` keyword present but not a table-definition list.
    for sql in [
        "CREATE TYPE my_type AS TABLE OF |",
        "CREATE OR REPLACE TYPE t AS TABLE OF | NUMBER",
        "ALTER INDEX idx REBUILD |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(!ctx.ddl_new_name_position, "{sql}");
        assert_eq!(
            ctx.phase,
            intellisense_context::SqlPhase::Initial,
            "phase for `{sql}`"
        );
    }

    // A prior statement must not bleed into the cursor's statement.
    let after_semicolon =
        analyze_inline_cursor_sql("SELECT * FROM dept; ALTER TABLE emp ADD PRIMARY KEY (|)");
    assert_eq!(
        after_semicolon.phase,
        intellisense_context::SqlPhase::DdlColumnList
    );
    assert_eq!(after_semicolon.focused_tables, vec!["emp".to_string()]);

    let ddl_then_select =
        analyze_inline_cursor_sql("ALTER TABLE emp ADD (a NUMBER); SELECT | FROM dept");
    assert!(!ddl_then_select.ddl_new_name_position);
    assert_eq!(ddl_then_select.phase, intellisense_context::SqlPhase::SelectList);
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
    let expr_keyword_ctx =
        SqlEditorWidget::expression_keyword_context(&deep_ctx, &data, &[], !prefix.is_empty(), Some(crate::db::DatabaseType::MySQL));
    let suggestions = SqlEditorWidget::base_suggestions_for_context(
        &mut data,
        &prefix,
        None,
        None,
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        context,
        false,
        Some(crate::db::DatabaseType::MySQL),
        expr_keyword_ctx,
    );

    (context, suggestions)
}

#[test]
fn constrained_object_slots_replace_base_catalog() {
    // A constrained DDL object slot resolves to a single object family (kind is
    // Some and not `Any`), which makes `trigger_intellisense` replace the base
    // catalog rather than append it — so `DROP TRIGGER`/`CALL`/`GRANT … ON`/
    // `GRANT … TO` no longer leak the whole catalog. `Any` slots (`AUDIT`) keep
    // it because every object kind is valid there.
    let build = || {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP_T".to_string()];
        data.views = vec!["EMP_V".to_string()];
        data.triggers = vec!["EMP_TRG".to_string()];
        data.indexes = vec!["EMP_IDX".to_string()];
        data.sequences = vec!["EMP_SEQ".to_string()];
        data.procedures = vec!["EMP_PROC".to_string()];
        data.functions = vec!["EMP_FN".to_string()];
        data.packages = vec!["EMP_PKG".to_string()];
        data.directories = vec!["EMP_DIR".to_string()];
        data.users = vec!["EMP_USR".to_string()];
        data.rebuild_indices();
        data
    };
    let analyze = |sql: &str| {
        let cursor = sql.find('|').unwrap();
        let s = sql.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&s, cursor);
        let mut data = build();
        let objs = SqlEditorWidget::collect_expected_object_suggestions(&mut data, &prefix, &ctx);
        let kind = SqlEditorWidget::expected_object_suggestion_kind(&prefix, None, &ctx);
        (kind, objs)
    };

    // Constrained slots: kind present and not Any, objects limited to the family
    // (+ users for schema qualification), no unrelated catalog kinds.
    let (k, objs) = analyze("DROP TRIGGER emp|");
    assert!(matches!(k, Some(kind) if kind != ExpectedObjectSuggestionKind::Any));
    assert!(objs.iter().any(|s| s == "EMP_TRG"));
    for leaked in ["EMP_T", "EMP_IDX", "EMP_DIR", "EMP_FN"] {
        assert!(!objs.iter().any(|s| s == leaked), "DROP TRIGGER leaked {leaked}: {objs:?}");
    }

    let (k, objs) = analyze("CALL emp|");
    assert!(matches!(k, Some(kind) if kind != ExpectedObjectSuggestionKind::Any));
    assert!(objs.iter().any(|s| s == "EMP_PROC") && objs.iter().any(|s| s == "EMP_FN"));
    assert!(!objs.iter().any(|s| s == "EMP_T"), "CALL leaked a table: {objs:?}");

    let (k, objs) = analyze("GRANT SELECT ON emp|");
    assert!(matches!(k, Some(kind) if kind != ExpectedObjectSuggestionKind::Any));
    assert!(objs.iter().any(|s| s == "EMP_T"));
    for leaked in ["EMP_TRG", "EMP_IDX", "EMP_DIR"] {
        assert!(!objs.iter().any(|s| s == leaked), "GRANT ON leaked {leaked}: {objs:?}");
    }

    // Grantee slot resolves to users only (previously dumped the whole catalog).
    for sql in ["GRANT SELECT ON emp_t TO emp|", "REVOKE SELECT ON emp_t FROM emp|"] {
        let (k, objs) = analyze(sql);
        assert_eq!(k, Some(ExpectedObjectSuggestionKind::User), "{sql}");
        assert_eq!(objs, vec!["EMP_USR".to_string()], "{sql}: {objs:?}");
    }

    // `AUDIT … ON` is an Any slot → kind is Any → base catalog is kept.
    let (k, _) = analyze("AUDIT SELECT ON emp|");
    assert_eq!(k, Some(ExpectedObjectSuggestionKind::Any));
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
fn expanded_statement_exact_bounds_ignores_plsql_words_inside_identifiers() {
    let statement = "SELECT begin_date, declared_at, package_body_label FROM audit_log";
    let expanded = ExpandedStatementWindow {
        statement_start: 8,
        statement_end: 8 + statement.len(),
        text: statement.to_string(),
        cursor_in_statement: statement.find("begin_date").unwrap_or(0),
    };
    let full_text = format!("SELECT 0;\n{statement};\nSELECT 1;");

    assert!(
        !SqlEditorWidget::expanded_statement_requires_exact_bounds(&full_text, &expanded),
        "identifier substrings should not force exact full-script statement bounds"
    );
}

#[test]
fn expanded_statement_exact_bounds_ignores_plsql_words_inside_literals_and_comments() {
    let statement = "SELECT 'BEGIN', col FROM emp -- DECLARE package body";
    let expanded = ExpandedStatementWindow {
        statement_start: 8,
        statement_end: 8 + statement.len(),
        text: statement.to_string(),
        cursor_in_statement: statement.find("col").unwrap_or(0),
    };
    let full_text = format!("SELECT 0;\n{statement};\nSELECT 1;");

    assert!(
        !SqlEditorWidget::expanded_statement_requires_exact_bounds(&full_text, &expanded),
        "literal/comment text should not force exact full-script statement bounds"
    );
}

#[test]
fn expanded_statement_exact_bounds_detects_real_plsql_tokens() {
    let procedure = "CREATE OR REPLACE PROCEDURE p IS\nBEGIN\n    NULL;\nEND;";
    let procedure_window = ExpandedStatementWindow {
        statement_start: 8,
        statement_end: 8 + procedure.len(),
        text: procedure.to_string(),
        cursor_in_statement: procedure.find("NULL").unwrap_or(0),
    };
    let full_text = format!("SELECT 0;\n{procedure}\n/\nSELECT 1;");
    assert!(
        SqlEditorWidget::expanded_statement_requires_exact_bounds(&full_text, &procedure_window),
        "real PL/SQL block tokens still require exact full-script statement bounds"
    );

    let package_body = "CREATE OR REPLACE PACKAGE /* editioned */ BODY p AS\nEND;";
    let package_window = ExpandedStatementWindow {
        statement_start: 8,
        statement_end: 8 + package_body.len(),
        text: package_body.to_string(),
        cursor_in_statement: package_body.find("BODY").unwrap_or(0),
    };
    let full_text = format!("SELECT 0;\n{package_body}\n/\nSELECT 1;");
    assert!(
        SqlEditorWidget::expanded_statement_requires_exact_bounds(&full_text, &package_window),
        "PACKAGE BODY tokens separated by comments still require exact bounds"
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
fn retarget_pending_intellisense_moves_caret_in_place() {
    // The fast-path filter advances the caret while a column load is still in
    // flight; retargeting keeps the load-completion refresh matching the new
    // caret so late-arriving comparison suggestions are still applied.
    let runtime = runtime_state_for_test(Some((4, 8)), Some(PendingIntellisense { cursor_pos: 8 }), 0, 0);

    runtime.retarget_pending_intellisense(9);

    assert_eq!(
        runtime.pending_intellisense().map(|pending| pending.cursor_pos),
        Some(9)
    );
}

#[test]
fn retarget_pending_intellisense_is_noop_without_pending_refresh() {
    // No load is in flight (suggestions were complete): retargeting must not
    // fabricate a refresh that would needlessly rebuild the popup.
    let runtime = runtime_state_for_test(Some((4, 8)), None, 0, 0);

    runtime.retarget_pending_intellisense(9);

    assert!(runtime.pending_intellisense().is_none());
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
fn collect_clause_wildcard_suggestions_only_at_projection_item_start() {
    // `*`/`t.*` name a whole-row projection, so they belong only at the start of
    // a fresh select-list item: right after `SELECT`, a set-quantifier, or a
    // list-separating comma — including inside a subquery's select list.
    let star = |sql: &str| SqlEditorWidget::collect_clause_wildcard_suggestions(
        "",
        None,
        &analyze_inline_cursor_sql(sql),
    );
    for sql in [
        "SELECT | FROM emp",
        "SELECT a, | FROM emp",
        "SELECT a, b, |, c FROM emp",
        "SELECT DISTINCT | FROM emp",
        "SELECT * FROM t WHERE id IN (SELECT | FROM u)",
        "SELECT * FROM t WHERE id IN (SELECT a, | FROM u)",
        "INSERT INTO t SELECT | FROM s",
    ] {
        assert!(
            star(sql).iter().any(|s| s == "*"),
            "expected `*` projection at item start for: {sql} (got {:?})",
            star(sql)
        );
    }

    // Anywhere else inside a select-list expression `*`/`t.*` is ungrammatical
    // noise — a `CASE` arm, or an operator operand.
    for sql in [
        "SELECT CASE WHEN | THEN 'y' END FROM t",
        "SELECT CASE a WHEN | THEN 'y' END FROM t",
        "SELECT CASE WHEN a = 1 THEN | END FROM t",
        "SELECT CASE WHEN a = 1 THEN 1 ELSE | END FROM t",
        "SELECT a + | FROM emp",
    ] {
        assert!(
            star(sql).is_empty(),
            "expected no wildcard mid-expression for: {sql} (got {:?})",
            star(sql)
        );
    }
}

#[test]
fn collect_clause_wildcard_suggestions_scope_to_innermost_nested_query() {
    // A `t.*` wildcard names a row source of the cursor's own query. In a nested
    // subquery — a scalar subquery in the SELECT list, an IN/EXISTS predicate
    // subquery, or any deeper nesting — that is the inner query's `FROM`, never
    // the enclosing query's tables.
    let star_scopes = |sql: &str| -> Vec<String> {
        SqlEditorWidget::collect_clause_wildcard_suggestions("", None, &analyze_inline_cursor_sql(sql))
            .into_iter()
            .filter(|s| s.ends_with(".*"))
            .collect()
    };

    assert_eq!(
        star_scopes("SELECT (SELECT | FROM b) FROM a"),
        vec!["b.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT * FROM t WHERE id IN (SELECT | FROM u)"),
        vec!["u.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT * FROM t WHERE EXISTS (SELECT | FROM u)"),
        vec!["u.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT * FROM emp e WHERE e.deptno = (SELECT | FROM dept d)"),
        vec!["d.*".to_string()]
    );
    // Nested two levels deep: only the innermost `FROM c` is in projection scope.
    assert_eq!(
        star_scopes("SELECT * FROM a WHERE x IN (SELECT y FROM b WHERE z IN (SELECT | FROM c))"),
        vec!["c.*".to_string()]
    );
    // An unclosed (mid-typing) nested subquery scopes to its inner `FROM` too.
    assert_eq!(
        star_scopes("SELECT (SELECT | FROM b"),
        vec!["b.*".to_string()]
    );
}

#[test]
fn collect_clause_wildcard_suggestions_scope_to_cursor_set_operation_branch() {
    // Each `UNION`/`INTERSECT`/`MINUS`/`EXCEPT` branch is an independent select
    // list with its own row sources, so a `t.*` wildcard names only the branch
    // the cursor is in — not the first branch.
    let star_scopes = |sql: &str| -> Vec<String> {
        SqlEditorWidget::collect_clause_wildcard_suggestions("", None, &analyze_inline_cursor_sql(sql))
            .into_iter()
            .filter(|s| s.ends_with(".*"))
            .collect()
    };

    assert_eq!(
        star_scopes("SELECT | FROM x UNION SELECT b FROM y"),
        vec!["x.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT a FROM x UNION SELECT | FROM y"),
        vec!["y.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT a FROM x UNION ALL SELECT | FROM y"),
        vec!["y.*".to_string()]
    );
    assert_eq!(
        star_scopes("SELECT a FROM x INTERSECT SELECT | FROM y"),
        vec!["y.*".to_string()]
    );
    // Third branch of a chain.
    assert_eq!(
        star_scopes("SELECT a FROM x UNION SELECT b FROM y UNION SELECT | FROM z"),
        vec!["z.*".to_string()]
    );
    // Set operation nested inside a predicate subquery.
    assert_eq!(
        star_scopes("SELECT * FROM t WHERE id IN (SELECT a FROM x UNION SELECT | FROM y)"),
        vec!["y.*".to_string()]
    );
}

#[test]
fn collect_clause_wildcard_suggestions_count_star_only_for_count_call() {
    // The bare `*` argument is grammatical only in `COUNT(*)` — the one function
    // that admits it. Every other call argument takes a value expression, so a
    // `*` there is noise.
    let star = |sql: &str| SqlEditorWidget::collect_clause_wildcard_suggestions(
        "",
        None,
        &analyze_inline_cursor_sql(sql),
    );

    assert_eq!(star("SELECT COUNT(|) FROM emp"), vec!["*".to_string()]);
    assert_eq!(star("SELECT count(|) FROM emp"), vec!["*".to_string()]);

    for sql in [
        "SELECT NVL(|, 0) FROM emp",
        "SELECT MAX(|) FROM emp",
        "SELECT SUM(|) FROM emp",
        "SELECT TRIM(|) FROM emp",
        // `COUNT(DISTINCT *)` is invalid — the cursor is no longer right after `(`.
        "SELECT COUNT(DISTINCT |) FROM emp",
    ] {
        assert!(
            star(sql).is_empty(),
            "expected no `*` argument for: {sql} (got {:?})",
            star(sql)
        );
    }
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
        ExpressionKeywordContext::ambiguous(),
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
        ExpressionKeywordContext::ambiguous(),
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
fn local_symbol_suggestions_exclude_sibling_and_post_cursor_scopes() {
    // Sibling nested block's variable must not leak into a later sibling block.
    let after_inner = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        "DECLARE v_outer NUMBER; BEGIN DECLARE v_inner NUMBER; BEGIN NULL; END; __CODEX_CURSOR__NULL; END;",
        &[],
    );
    assert_has_case_insensitive(&after_inner, "v_outer");
    assert!(
        !after_inner.iter().any(|s| s.eq_ignore_ascii_case("v_inner")),
        "closed sibling block local leaked: {after_inner:?}"
    );

    // A package body's other procedure local must not leak across routines.
    let proc_b = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        "CREATE PACKAGE BODY p IS PROCEDURE a IS v_aa NUMBER; BEGIN NULL; END; PROCEDURE b IS v_bb NUMBER; BEGIN __CODEX_CURSOR__NULL; END; END;",
        &[],
    );
    assert_has_case_insensitive(&proc_b, "v_bb");
    assert!(
        !proc_b.iter().any(|s| s.eq_ignore_ascii_case("v_aa")),
        "sibling procedure local leaked: {proc_b:?}"
    );

    // A variable declared after the cursor is not yet in scope.
    let pre_decl = SqlEditorWidget::collect_local_symbol_suggestions_for_test(
        "DECLARE v_a NUMBER; BEGIN __CODEX_CURSOR__NULL; v_b := 1; END;",
        &[],
    );
    assert_has_case_insensitive(&pre_decl, "v_a");
    assert!(
        !pre_decl.iter().any(|s| s.eq_ignore_ascii_case("v_b")),
        "post-cursor symbol leaked: {pre_decl:?}"
    );
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

    assert_has_case_insensitive(&columns, "avg_sal_calc");
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
fn classify_intellisense_context_treats_create_index_column_list_as_column_context() {
    let deep_ctx = analyze_inline_cursor_sql("CREATE INDEX ix ON target (|)");
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::DdlColumnList);
    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

#[test]
fn classify_intellisense_context_treats_alter_table_drop_column_as_column_context() {
    let deep_ctx = analyze_inline_cursor_sql("ALTER TABLE target DROP COLUMN |");
    assert_eq!(deep_ctx.phase, intellisense_context::SqlPhase::DdlColumnList);
    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

/// `ALTER TABLE t ADD (…)` is a column-definition list, not a table target.
/// Each entry start is a brand-new column name (suppress identifiers) and a
/// position after the name is a data-type slot; neither should offer existing
/// relations or columns. Previously the parenthesised form fell through to the
/// DML machine's `IntoClause`/`DerivedAliasColumnList`, leaking table names and
/// the wrong table's columns.
#[test]
fn alter_table_add_column_definition_slots_are_new_name_positions() {
    for sql in [
        "ALTER TABLE emp ADD (|)",
        "ALTER TABLE emp ADD (col1 NUMBER, |)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            ctx.ddl_new_name_position,
            "`{sql}` should be a DDL new-name position"
        );
    }

    // The slot after the new column name stays a data-type position (not a
    // name position), so dialect type keywords still appear there.
    let type_slot = analyze_inline_cursor_sql("ALTER TABLE emp ADD (col1 |)");
    assert!(!type_slot.ddl_new_name_position);
    assert!(SqlEditorWidget::data_type_position_for_context(&type_slot, false).is_some());
}

/// Constraint and `REFERENCES` column lists inside `ALTER TABLE … ADD …`
/// reference existing columns of a single table — the altered table for
/// `PRIMARY KEY`/`UNIQUE`/`FOREIGN KEY`, and the referenced table for
/// `REFERENCES x (…)`. They must classify as a column context focused on that
/// table, never offer relations.
#[test]
fn alter_table_add_constraint_column_lists_target_existing_columns() {
    for sql in [
        "ALTER TABLE emp ADD PRIMARY KEY (|)",
        "ALTER TABLE emp ADD UNIQUE (|)",
        "ALTER TABLE emp ADD CONSTRAINT pk PRIMARY KEY (|)",
        "ALTER TABLE emp ADD FOREIGN KEY (|) REFERENCES dept(id)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert_eq!(
            ctx.phase,
            intellisense_context::SqlPhase::DdlColumnList,
            "phase for `{sql}`"
        );
        assert_eq!(ctx.focused_tables, vec!["emp".to_string()], "{sql}");
    }

    // The `REFERENCES <table> (…)` list targets the referenced table's columns.
    let references =
        analyze_inline_cursor_sql("ALTER TABLE emp ADD FOREIGN KEY (deptno) REFERENCES dept(|)");
    assert_eq!(references.phase, intellisense_context::SqlPhase::DdlColumnList);
    assert_eq!(references.focused_tables, vec!["dept".to_string()]);

    // A schema-qualified target keeps the qualifier so the right table's
    // columns are loaded.
    let qualified = analyze_inline_cursor_sql("ALTER TABLE scott.emp ADD PRIMARY KEY (|)");
    assert_eq!(qualified.phase, intellisense_context::SqlPhase::DdlColumnList);
    assert_eq!(qualified.focused_tables, vec!["scott.emp".to_string()]);
}

/// A `CHECK (…)` constraint is an expression over the altered table's columns,
/// so it must offer those columns — never relations. A type precision/size
/// argument (`NUMBER(…)`) is a numeric-literal slot and must stay suppressed
/// rather than leak columns. Both previously fell through to the DML machine.
#[test]
fn alter_table_add_check_and_type_precision_are_classified_precisely() {
    for sql in [
        "ALTER TABLE emp ADD CHECK (| > 0)",
        "ALTER TABLE emp ADD CONSTRAINT c CHECK (|)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert_eq!(
            ctx.phase,
            intellisense_context::SqlPhase::DdlColumnList,
            "phase for `{sql}`"
        );
        assert_eq!(ctx.focused_tables, vec!["emp".to_string()], "{sql}");
    }

    // A type precision/size argument inside the definition list is a literal
    // position: suppressed, never the catalog or the altered table's columns.
    let precision = analyze_inline_cursor_sql("ALTER TABLE emp ADD (col1 NUMBER(|))");
    assert!(precision.ddl_new_name_position);
}

/// MySQL/MariaDB allow an index or constraint name between the `KEY`/`INDEX`
/// keyword and its column list (`ADD KEY idx (col)`, `ADD INDEX idx (col)`,
/// `ADD UNIQUE KEY uk (col)`, `ADD … FOREIGN KEY fk (col)`). The intervening
/// name must not break the column-list classification into a table target.
#[test]
fn alter_table_add_named_index_column_lists_target_existing_columns() {
    for sql in [
        "ALTER TABLE emp ADD INDEX idx (|)",
        "ALTER TABLE emp ADD KEY idx (|)",
        "ALTER TABLE emp ADD UNIQUE KEY uk (|)",
        "ALTER TABLE emp ADD CONSTRAINT fk FOREIGN KEY fk_name (|) REFERENCES dept(id)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert_eq!(
            ctx.phase,
            intellisense_context::SqlPhase::DdlColumnList,
            "phase for `{sql}`"
        );
        assert_eq!(ctx.focused_tables, vec!["emp".to_string()], "{sql}");
    }
}

/// MySQL `CHANGE [COLUMN] old new …` names a brand-new column after the source
/// name; that slot (and the data type after it) must suppress identifiers
/// rather than offer existing relations. The source-name slot itself stays an
/// existing-column position.
#[test]
fn alter_table_change_new_name_slot_is_suppressed() {
    for sql in [
        "ALTER TABLE emp CHANGE old_col |",
        "ALTER TABLE emp CHANGE COLUMN old_col |",
        "ALTER TABLE emp CHANGE old_col new_col |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(ctx.ddl_new_name_position, "`{sql}` should suppress");
    }

    // The source-column slot is still an existing-column position, not a new
    // name — including after the optional `COLUMN` keyword.
    for sql in ["ALTER TABLE emp CHANGE |", "ALTER TABLE emp CHANGE COLUMN |"] {
        let source = analyze_inline_cursor_sql(sql);
        assert!(!source.ddl_new_name_position, "{sql}");
        assert_eq!(
            source.phase,
            intellisense_context::SqlPhase::DdlColumnList,
            "{sql}"
        );
        assert_eq!(source.focused_tables, vec!["emp".to_string()], "{sql}");
    }
}

/// `CREATE TABLE … PARTITION BY {RANGE|HASH|LIST|RANGE COLUMNS} (…)` lists the
/// partitioning columns/expressions over the table's own columns — not a table
/// target. For `CREATE TABLE` the columns are defined in the same statement, so
/// the slot is suppressed rather than leaking the catalog. The partition-value
/// parens that follow must keep their ordinary classification.
#[test]
fn create_table_partition_column_list_never_offers_relations() {
    for sql in [
        "CREATE TABLE t (id NUMBER) PARTITION BY RANGE (|)",
        "CREATE TABLE t (id INT) PARTITION BY HASH (|)",
        "CREATE TABLE t (id INT) PARTITION BY LIST (|)",
        "CREATE TABLE t (a INT, b INT) PARTITION BY RANGE COLUMNS (|)",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            ctx.ddl_new_name_position,
            "`{sql}` partition column list should suppress relations"
        );
    }

    // The partition-value paren after the column list is not a column list.
    let bound = analyze_inline_cursor_sql(
        "CREATE TABLE t (id INT) PARTITION BY RANGE (id) (PARTITION p VALUES LESS THAN (|))",
    );
    assert!(!bound.ddl_new_name_position);
}

/// `CREATE TABLE … AS SELECT …` (CTAS) has no column-definition list: its
/// subquery and expression parens are ordinary query positions and must keep
/// their query classification, not be mistaken for a definition list. Guards
/// the "first paren after the table name" definition-list heuristic.
#[test]
fn create_table_as_select_parens_are_not_definition_lists() {
    let expr = analyze_inline_cursor_sql("CREATE TABLE t AS SELECT (| ) FROM dept");
    assert!(!expr.ddl_new_name_position);
    assert_eq!(expr.phase, intellisense_context::SqlPhase::SelectList);

    let subquery = analyze_inline_cursor_sql(
        "CREATE TABLE t AS SELECT * FROM dept WHERE deptno IN (SELECT | FROM emp)",
    );
    assert!(!subquery.ddl_new_name_position);
    assert_eq!(subquery.phase, intellisense_context::SqlPhase::SelectList);

    // A real definition list followed by `AS SELECT` still classifies its body
    // as a query.
    let mixed = analyze_inline_cursor_sql("CREATE TABLE t (id NUMBER) AS SELECT | FROM dept");
    assert!(!mixed.ddl_new_name_position);
    assert_eq!(mixed.phase, intellisense_context::SqlPhase::SelectList);
}

/// `CREATE TABLE (…)` column definitions name brand-new columns; the position
/// is a new-name slot, not a table target. Constraint sub-lists reference
/// columns defined in the same (not-yet-existing) statement, so they are left a
/// name position rather than leaking the catalog. The data-type slot is
/// unaffected.
#[test]
fn create_table_definition_slots_never_offer_relations() {
    for sql in [
        "CREATE TABLE t (|)",
        "CREATE TABLE t (id NUMBER, |)",
        "CREATE TABLE t (id NUMBER, PRIMARY KEY (|))",
        "CREATE TABLE t (id NUMBER, CONSTRAINT pk PRIMARY KEY (|))",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            ctx.ddl_new_name_position,
            "`{sql}` should be a DDL new-name position"
        );
    }

    let type_slot = analyze_inline_cursor_sql("CREATE TABLE t (id NUMBER, name |)");
    assert!(!type_slot.ddl_new_name_position);
    assert!(SqlEditorWidget::data_type_position_for_context(&type_slot, false).is_some());
}

/// `ALTER TABLE … MODIFY/DROP/RENAME …` (existing-column operations) and a
/// plain `CREATE TABLE … AS SELECT` must keep their established classification —
/// the new definition-list detector only governs `ADD`/`CREATE TABLE (…)` lists.
#[test]
fn ddl_definition_list_detector_leaves_other_alter_operations_intact() {
    for (sql, expected) in [
        ("ALTER TABLE emp MODIFY (|)", intellisense_context::SqlPhase::DdlColumnList),
        ("ALTER TABLE emp MODIFY |", intellisense_context::SqlPhase::DdlColumnList),
        ("ALTER TABLE emp RENAME COLUMN | TO x", intellisense_context::SqlPhase::DdlColumnList),
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert_eq!(ctx.phase, expected, "phase for `{sql}`");
        assert!(!ctx.ddl_new_name_position, "{sql}");
        assert_eq!(ctx.focused_tables, vec!["emp".to_string()], "{sql}");
    }

    // CTAS body is an ordinary SELECT list, untouched by the detector.
    let ctas = analyze_inline_cursor_sql("CREATE TABLE t AS SELECT | FROM emp");
    assert_eq!(ctas.phase, intellisense_context::SqlPhase::SelectList);
    assert!(!ctas.ddl_new_name_position);
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
        let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None);
        let expected: Vec<String> = expected.iter().map(|value| (*value).to_string()).collect();
        assert_eq!(suggestions, expected, "sql: {sql}");
    }

    let when_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &when_ctx, None);

    assert!(when_suggestions.iter().any(|value| value == "MATCHED"));
    assert!(when_suggestions.iter().any(|value| value == "NOT"));
}

#[test]
fn extract_field_slot_suppresses_columns_and_offers_field_keywords() {
    let at_field = |sql: &str| {
        SqlEditorWidget::extract_field_position_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
        .is_some()
    };
    // Field slot (`EXTRACT(|`) and the awaiting-FROM slot (`EXTRACT(YEAR |`) both
    // suppress columns — a column is never valid before the FROM.
    assert!(at_field("SELECT EXTRACT(| FROM hire_date) FROM emp"));
    assert!(at_field("SELECT EXTRACT(YEAR | FROM hire_date) FROM emp"));
    // The source expression after FROM is a real column position.
    assert!(!at_field("SELECT EXTRACT(YEAR FROM |) FROM emp"));
    assert!(!at_field("SELECT EXTRACT(YEAR FROM hire_date) , | FROM emp"));
    // An ordinary column position / a non-EXTRACT function call is unaffected.
    assert!(!at_field("SELECT | FROM emp"));
    assert!(!at_field("SELECT TRUNC(| ) FROM emp"));

    // Oracle field keywords are offered at the field slot; MySQL has its own set.
    let oracle_fields = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT EXTRACT(| FROM hire_date) FROM emp"),
        Some(crate::db::DatabaseType::Oracle),
    );
    assert!(oracle_fields.iter().any(|value| value == "YEAR"));
    assert!(oracle_fields.iter().any(|value| value == "TIMEZONE_HOUR"));
    let mysql_fields = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT EXTRACT(| FROM hire_date) FROM emp"),
        Some(crate::db::DatabaseType::MySQL),
    );
    assert!(mysql_fields.iter().any(|value| value == "QUARTER"));
    assert!(mysql_fields.iter().any(|value| value == "YEAR_MONTH"));
    // After the field name, `FROM` is offered.
    let awaiting_from = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT EXTRACT(YEAR | FROM hire_date) FROM emp"),
        Some(crate::db::DatabaseType::Oracle),
    );
    assert_eq!(awaiting_from, vec!["FROM".to_string()]);
}

#[test]
fn json_returning_type_slot_offers_types_and_suppresses_columns() {
    // The JSON `RETURNING` type slot routes through the shared keyword-only
    // chokepoint, so columns are suppressed there.
    let at_slot = |sql: &str| {
        SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    assert!(at_slot("SELECT JSON_VALUE(doc, '$.a' RETURNING |) FROM t"));
    assert!(at_slot("SELECT JSON_QUERY(doc, '$.a' RETURNING |) FROM t"));
    // A statement-level DML RETURNING lists columns, not types — not a type slot.
    assert!(!at_slot("UPDATE t SET x = 1 RETURNING | INTO :v"));
    assert!(!at_slot("DELETE FROM t WHERE id = 1 RETURNING |"));

    // Dialect-correct type keywords are offered at the JSON RETURNING slot.
    let oracle = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT JSON_VALUE(doc, '$.a' RETURNING |) FROM t"),
        Some(crate::db::DatabaseType::Oracle),
    );
    assert!(oracle.iter().any(|value| value == "VARCHAR2"));
    assert!(oracle.iter().any(|value| value == "NUMBER"));
    let mysql = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT JSON_VALUE(doc, '$.a' RETURNING |) FROM t"),
        Some(crate::db::DatabaseType::MySQL),
    );
    assert!(mysql.iter().any(|value| value == "UNSIGNED"));
}

#[test]
fn clause_continuation_keyword_is_suppressed_after_qualified_member() {
    let suggests = |sql: &str, kw: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
        .iter()
        .any(|value| value == kw)
    };
    // A standalone clause keyword still offers its continuation.
    assert!(suggests("SELECT * FROM emp ORDER |", "BY"));
    assert!(suggests("SELECT * FROM emp GROUP |", "BY"));
    assert!(suggests("SELECT * FROM emp CONNECT |", "BY"));
    assert!(suggests("SELECT * FROM emp START |", "WITH"));
    // A qualified member named order/group/start is a column — no continuation.
    assert!(!suggests("SELECT * FROM emp e WHERE e.order |", "BY"));
    assert!(!suggests("SELECT * FROM emp e WHERE e.group |", "BY"));
    assert!(!suggests("SELECT * FROM emp e WHERE e.start |", "WITH"));
}

#[test]
fn join_continuation_keyword_is_scoped_to_table_context() {
    let suggests_join = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
        .iter()
        .any(|value| value == "JOIN")
    };
    // In a table position the join continuation is still offered.
    assert!(suggests_join("SELECT * FROM a LEFT |"));
    assert!(suggests_join("SELECT * FROM a INNER |"));
    assert!(suggests_join("SELECT * FROM a NATURAL |"));
    assert!(suggests_join("SELECT * FROM a CROSS |"));
    // A column/function named left/right in an expression position must not.
    assert!(!suggests_join("SELECT left | FROM a"));
    assert!(!suggests_join("SELECT * FROM a WHERE right | "));
}

#[test]
fn column_suppressing_keyword_slot_covers_every_family() {
    let at = |sql: &str| {
        SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    // One representative of each keyword/value-only family routes through the
    // single chokepoint, so suppression can never drift from keyword emission.
    assert!(at("SELECT CAST(x AS |) FROM t")); // data type
    assert!(at("SELECT * FROM t ORDER BY id FETCH FIRST |")); // row limiting
    assert!(at("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED |) FROM t")); // window frame
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING |) FROM t"
    )); // completed first frame bound
    assert!(at("SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW |) FROM t")); // completed frame bound
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE NO |) FROM t"
    )); // EXCLUDE tail
    assert!(at("SELECT max(empno) KEEP (DENSE_RANK |) FROM emp")); // KEEP dense-rank slot
    assert!(at("SELECT EXTRACT(| FROM hire_date) FROM emp")); // EXTRACT field
    assert!(at("SELECT hire_date - INTERVAL '5' | FROM emp")); // INTERVAL unit
    assert!(at("SELECT * FROM emp ORDER BY id ASC |")); // ORDER BY sort modifier
    assert!(at("SELECT * FROM emp ORDER BY id NULLS FIRST |")); // completed sort modifier
    assert!(at("SELECT * FROM emp ORDER |")); // pure clause-keyword continuation
    assert!(at("SELECT * FROM a LEFT |")); // pure join-type continuation
    assert!(at("SELECT sum(x) OVER (PARTITION |) FROM t")); // window PARTITION BY
    assert!(at("SELECT * FROM emp WHERE a IS |")); // IS null-test operator
    assert!(at("SELECT * FROM emp WHERE a IS NOT |"));
    // Ordinary column positions remain column positions.
    assert!(!at("SELECT | FROM emp"));
    assert!(!at("SELECT * FROM emp WHERE | "));
    // Value-bound window-frame slots accept an expression, so they are NOT here.
    assert!(!at("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN | ) FROM t"));
}

#[test]
fn pure_clause_keyword_continuation_covers_every_family() {
    let at = |sql: &str| {
        SqlEditorWidget::cursor_is_at_pure_clause_keyword_continuation_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    // Multi-word clause openers whose trailing keyword has not been typed: only
    // that keyword is grammatical, so every identifier source is suppressed.
    assert!(at("SELECT * FROM t ORDER |")); // -> BY
    assert!(at("SELECT * FROM t GROUP |")); // -> BY
    assert!(at("SELECT * FROM t CONNECT |")); // -> BY
    assert!(at("SELECT * FROM t ORDER SIBLINGS |")); // -> BY
    assert!(at("SELECT * FROM t START |")); // -> WITH
    assert!(at("SELECT * FROM a LEFT |")); // -> JOIN
    assert!(at("SELECT * FROM a INNER |"));
    assert!(at("SELECT * FROM a CROSS |"));
    assert!(at("SELECT * FROM a NATURAL |"));
    assert!(at("SELECT * FROM a LEFT OUTER |")); // -> JOIN
    assert!(at("SELECT * FROM a FULL OUTER |"));
    // The clause openers suppress in a column context too (a complete predicate
    // precedes the bare ORDER), not only in the FROM table context.
    assert!(at("SELECT a FROM t WHERE a = 1 ORDER |"));
    assert!(at("SELECT a FROM t GROUP BY a ORDER |"));

    // `PARTITION |` continues to `BY` only inside an analytic window spec, so it
    // is a continuation there (`OVER (...)` and `WINDOW name AS (...)`) …
    assert!(at("SELECT sum(x) OVER (PARTITION |) FROM t"));
    assert!(at("SELECT count(*) FROM t WINDOW w AS (PARTITION |)"));
    // … but not where `PARTITION` introduces a partition name: a partition-
    // extended table reference, a partitioned outer join, or a DDL maintenance op.
    assert!(!at("SELECT * FROM sales PARTITION |"));
    assert!(!at("SELECT * FROM sales s PARTITION |"));
    assert!(!at("ALTER TABLE t DROP PARTITION |"));

    // Not a continuation: the trailing keyword is already present, an ordinary
    // clause body, a typed prefix, a qualified member column, a join-type word in
    // an expression, or a completed join target.
    assert!(!at("SELECT * FROM t ORDER BY |"));
    assert!(!at("SELECT * FROM t ORDER BY id ASC |"));
    assert!(!at("SELECT * FROM t ORDER BY id NULLS |"));
    assert!(!at("SELECT * FROM t ORDER BY id NULLS FIRST |"));
    assert!(!at("SELECT | FROM t"));
    assert!(!at("SELECT * FROM t WHERE |"));
    assert!(!at("SELECT * FROM t |"));
    assert!(!at("SELECT * FROM t ORD|"));
    assert!(!at("SELECT left | FROM t"));
    assert!(!at("SELECT * FROM t e WHERE e.order |"));
    assert!(!at("SELECT * FROM a LEFT OUTER JOIN b |"));
}

#[test]
fn outer_join_continuation_offers_join_keyword() {
    let suggests_join = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
        .iter()
        .any(|value| value == "JOIN")
    };
    // `<side> OUTER` is a join continuation just like the bare join types, so it
    // must offer `JOIN` (and, via the shared continuation predicate, suppress the
    // relation list that would otherwise leak there).
    assert!(suggests_join("SELECT * FROM a LEFT OUTER |"));
    assert!(suggests_join("SELECT * FROM a RIGHT OUTER |"));
    assert!(suggests_join("SELECT * FROM a FULL OUTER |"));
    // Still scoped to a table context.
    assert!(!suggests_join("SELECT outer | FROM a"));

    // `LEFT`/`RIGHT`/`FULL` additionally offer the optional `OUTER` before
    // `JOIN`, so typing it mid-keyword (`LEFT O|`) stays useful; `INNER`/`CROSS`/
    // `NATURAL` do not take `OUTER`.
    let candidates = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
    };
    assert!(candidates("SELECT * FROM a LEFT |").iter().any(|v| v == "OUTER"));
    assert!(candidates("SELECT * FROM a FULL |").iter().any(|v| v == "OUTER"));
    assert!(!candidates("SELECT * FROM a INNER |")
        .iter()
        .any(|v| v == "OUTER"));
}

#[test]
fn window_partition_continuation_offers_by_and_suppresses_columns() {
    let suggests_by = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
        .iter()
        .any(|value| value == "BY")
    };
    // Inside an analytic spec, `PARTITION |` offers `BY`.
    assert!(suggests_by("SELECT sum(x) OVER (PARTITION |) FROM t"));
    assert!(suggests_by("SELECT count(*) FROM t WINDOW w AS (PARTITION |)"));
    // A non-window `PARTITION` expects a partition name, never `BY`.
    assert!(!suggests_by("SELECT * FROM sales PARTITION |"));
    assert!(!suggests_by("ALTER TABLE t DROP PARTITION |"));
}

#[test]
fn is_null_test_continuation_offers_keywords_and_suppresses_columns() {
    let at = |sql: &str| {
        SqlEditorWidget::cursor_is_at_is_null_test_keyword_position_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    let kw = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
    };
    // `IS |` -> NOT / NULL; `IS NOT |` -> NULL. Identifiers are never valid after
    // `IS`, so the position routes through the column-suppression chokepoint.
    assert!(at("SELECT * FROM t WHERE a IS |"));
    assert_eq!(kw("SELECT * FROM t WHERE a IS |"), vec!["NOT", "NULL"]);
    assert!(at("SELECT * FROM t WHERE a IS NOT |"));
    assert_eq!(kw("SELECT * FROM t WHERE a IS NOT |"), vec!["NULL"]);
    // Works for any operand and in any clause, including a chained predicate and a
    // boolean expression in the select list.
    assert!(at("SELECT * FROM t WHERE f(x) IS |"));
    assert!(at("SELECT * FROM t WHERE a IS NULL AND b IS |"));
    assert!(at("SELECT a IS | FROM t"));

    // Not the operator: a completed `IS NULL`, an ordinary operand position, the
    // `<col> NOT` predicate tail, a qualified member column named `is`, and the
    // continuation after a finished predicate.
    assert!(!at("SELECT * FROM t WHERE a IS NULL |"));
    assert!(!at("SELECT * FROM t WHERE a |"));
    assert!(!at("SELECT * FROM t WHERE a NOT |"));
    assert!(!at("SELECT * FROM t e WHERE e.is |"));
    assert!(!at("SELECT * FROM t WHERE a IS NULL AND b |"));
}

#[test]
fn dml_target_continuation_offers_structural_keyword_and_suppresses_relations() {
    let det = |sql: &str| {
        SqlEditorWidget::cursor_is_after_complete_dml_target_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    let kw = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    // After a complete DML target table, only the following clause keyword is
    // grammatical — never another relation — so the position both offers that
    // keyword and routes through the identifier-suppression flag.
    assert!(det("UPDATE emp |"));
    assert_eq!(kw("UPDATE emp |"), vec!["SET"]);
    assert!(det("UPDATE emp e |")); // after the alias too
    assert!(det("UPDATE schema.emp |")); // schema-qualified target
    assert!(det("DELETE FROM emp |"));
    assert_eq!(kw("DELETE FROM emp |"), vec!["WHERE"]);
    assert!(det("DELETE FROM emp e |"));
    assert!(det("INSERT INTO emp |"));
    assert_eq!(kw("INSERT INTO emp |"), vec!["VALUES", "SELECT"]);
    assert!(det("MERGE INTO emp |"));
    assert_eq!(kw("MERGE INTO emp |"), vec!["USING"]);
    assert!(det("MERGE INTO emp e |"));

    // The target is not yet complete (still the leading keyword), so the table
    // list must keep flowing.
    assert!(!det("UPDATE |"));
    assert!(!det("DELETE FROM |"));
    assert!(!det("INSERT INTO |"));
    assert!(!det("MERGE INTO |"));

    // A SELECT `FROM` shares the phase but is not a DML target — table completion
    // (including comma-separated cross joins) stays intact.
    assert!(!det("SELECT * FROM emp |"));
    assert!(!det("SELECT * FROM a, |"));
    assert!(!det("SELECT * FROM t WHERE x IN (SELECT a FROM u |)"));

    // MySQL multi-table / join forms still expect another relation, so the simple
    // single-target heuristics must not fire: a trailing comma, or a join target.
    assert!(!det("UPDATE t1, |"));
    assert!(!det("DELETE t1 FROM t1 JOIN t2 |"));
    assert!(!det("DELETE FROM emp e JOIN x |"));

    // Once the structural keyword is present the dedicated clause phase takes over.
    assert!(!det("UPDATE emp SET a = 1 |"));
    assert!(!det("DELETE FROM emp WHERE |"));
    assert!(!det("INSERT INTO emp VALUES |"));

    // `CREATE INDEX ... ON t` shares the phase but is not a DML target.
    assert!(!det("CREATE INDEX i ON t |"));
}

#[test]
fn join_target_continuation_offers_on_using_and_suppresses_relations() {
    let det = |sql: &str| {
        SqlEditorWidget::cursor_is_after_complete_join_target_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    let kw = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    // After a complete JOIN target table, the join condition keywords are
    // grammatical — never another relation — so the slot offers them and routes
    // through the identifier-suppression flag.
    assert!(det("SELECT * FROM a JOIN b |"));
    assert_eq!(kw("SELECT * FROM a JOIN b |"), vec!["ON", "USING"]);
    assert!(det("SELECT * FROM a JOIN b x |")); // after the alias too
    assert!(det("SELECT * FROM a LEFT JOIN b |"));
    assert!(det("SELECT * FROM a LEFT OUTER JOIN b |"));
    assert!(det("SELECT * FROM a INNER JOIN b |"));
    assert!(det("SELECT * FROM a JOIN sch.b |")); // schema-qualified target
    assert!(det("SELECT * FROM a JOIN b ON a.x = b.y JOIN c |")); // chained join
    assert!(det("SELECT * FROM (SELECT 1 x) z JOIN b |")); // subquery left side
    assert!(det("SELECT 1 FROM u JOIN v |")); // inside a subquery scope

    // `CROSS`/`NATURAL` joins take no condition keyword.
    assert!(!det("SELECT * FROM a CROSS JOIN b |"));
    assert!(!det("SELECT * FROM a NATURAL JOIN b |"));

    // The target is not yet present, the condition has already started, or the
    // cursor is in an ordinary FROM/comma position — table completion stays intact.
    assert!(!det("SELECT * FROM a JOIN |"));
    assert!(!det("SELECT * FROM a JOIN b ON |"));
    assert!(!det("SELECT * FROM a JOIN b ON a.x = b.y |"));
    assert!(!det("SELECT * FROM a JOIN b USING |"));
    assert!(!det("SELECT * FROM a JOIN b USING (c) |"));
    assert!(!det("SELECT * FROM a |"));
    assert!(!det("SELECT * FROM a, b |"));
    // A bare join-type word routes through the join-continuation family instead.
    assert!(!det("SELECT * FROM a LEFT |"));
}

#[test]
fn interval_unit_slot_suppresses_columns_and_offers_unit_keywords() {
    let at_unit = |sql: &str| {
        SqlEditorWidget::interval_unit_position_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
        .is_some()
    };
    // Every qualifier slot of a quoted INTERVAL literal suppresses columns.
    assert!(at_unit("SELECT hire_date - INTERVAL '5' | FROM emp"));
    assert!(at_unit("SELECT hire_date - INTERVAL '5' DAY | FROM emp"));
    assert!(at_unit("SELECT hire_date - INTERVAL '5' DAY TO | FROM emp"));
    assert!(at_unit("SELECT hire_date + INTERVAL '1-2' YEAR TO | FROM emp"));
    // MySQL's unquoted `INTERVAL <expr> <unit>` keeps the expr a column position.
    assert!(!at_unit("SELECT hire_date - INTERVAL | DAY FROM emp"));
    // Ordinary expression positions are unaffected.
    assert!(!at_unit("SELECT | FROM emp"));
    assert!(!at_unit("SELECT hire_date - | FROM emp"));

    // Oracle leading units, then `TO`, then trailing units.
    let oracle = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    let leading = oracle("SELECT hire_date - INTERVAL '5' | FROM emp");
    assert!(leading.iter().any(|value| value == "DAY"));
    assert!(leading.iter().any(|value| value == "SECOND"));
    assert_eq!(
        oracle("SELECT hire_date - INTERVAL '5' DAY | FROM emp"),
        vec!["TO".to_string()]
    );
    let trailing = oracle("SELECT hire_date - INTERVAL '5' DAY TO | FROM emp");
    assert!(trailing.iter().any(|value| value == "SECOND"));
    assert!(!trailing.iter().any(|value| value == "DAY"));
    // MySQL offers its own unit names at the leading slot and no `TO`.
    let mysql_leading = SqlEditorWidget::collect_expected_keyword_suggestions(
        "",
        &analyze_inline_cursor_sql("SELECT hire_date - INTERVAL '5' | FROM emp"),
        Some(crate::db::DatabaseType::MySQL),
    );
    assert!(mysql_leading.iter().any(|value| value == "QUARTER"));
}

#[test]
fn merge_when_matched_keyword_is_scoped_to_merge_action_slot() {
    let suggests_matched = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            None,
        )
        .iter()
        .any(|value| value == "MATCHED")
    };
    // Real MERGE merge-action slots still offer MATCHED.
    assert!(suggests_matched(
        "MERGE INTO target t USING src s ON (t.id = s.id) WHEN |"
    ));
    assert!(suggests_matched(
        "MERGE INTO target t USING src s ON (t.id = s.id) WHEN NOT |"
    ));
    // A `CASE WHEN` branch is never a merge-action slot: no MATCHED, even when
    // the CASE lives inside a MERGE's SET/INSERT expression.
    assert!(!suggests_matched("SELECT CASE WHEN | END FROM t"));
    assert!(!suggests_matched("SELECT CASE WHEN NOT | END FROM t"));
    assert!(!suggests_matched(
        "MERGE INTO target t USING src s ON (t.id = s.id) \
         WHEN MATCHED THEN UPDATE SET t.v = CASE WHEN |"
    ));
    // PL/SQL searched CASE statement is likewise not a merge slot.
    assert!(!suggests_matched("BEGIN CASE WHEN | THEN NULL; END CASE; END;"));
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
    let drop_ctx = analyze_inline_cursor_sql("DROP |");
    let drop_public_ctx = analyze_inline_cursor_sql("DROP PUBLIC |");
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE B|");
    let create_unique_ctx = analyze_inline_cursor_sql("CREATE UNIQUE |");
    let create_bitmap_ctx = analyze_inline_cursor_sql("CREATE BITMAP |");
    let create_global_ctx = analyze_inline_cursor_sql("CREATE GLOBAL |");
    let create_global_temporary_ctx = analyze_inline_cursor_sql("CREATE GLOBAL TEMPORARY |");
    let create_public_ctx = analyze_inline_cursor_sql("CREATE PUBLIC |");
    let create_or_replace_public_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE PUBLIC |");
    let create_database_ctx = analyze_inline_cursor_sql("CREATE DATABASE |");
    let create_or_replace_database_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE DATABASE |");
    let create_public_database_ctx = analyze_inline_cursor_sql("CREATE PUBLIC DATABASE |");
    let create_shared_ctx = analyze_inline_cursor_sql("CREATE SHARED |");
    let create_or_replace_shared_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE SHARED |");
    let create_shared_database_ctx = analyze_inline_cursor_sql("CREATE SHARED DATABASE |");
    let create_shared_public_ctx = analyze_inline_cursor_sql("CREATE SHARED PUBLIC |");
    let drop_database_ctx = analyze_inline_cursor_sql("DROP DATABASE |");
    let drop_public_database_ctx = analyze_inline_cursor_sql("DROP PUBLIC DATABASE |");
    let drop_shared_ctx = analyze_inline_cursor_sql("DROP SHARED |");
    let alter_ctx = analyze_inline_cursor_sql("ALTER |");
    let alter_public_ctx = analyze_inline_cursor_sql("ALTER PUBLIC |");
    let alter_database_ctx = analyze_inline_cursor_sql("ALTER DATABASE |");
    let alter_public_database_ctx = analyze_inline_cursor_sql("ALTER PUBLIC DATABASE |");
    let alter_shared_ctx = analyze_inline_cursor_sql("ALTER SHARED |");
    let alter_shared_database_ctx = analyze_inline_cursor_sql("ALTER SHARED DATABASE |");
    let alter_shared_public_ctx = analyze_inline_cursor_sql("ALTER SHARED PUBLIC |");
    let alter_shared_public_database_ctx =
        analyze_inline_cursor_sql("ALTER SHARED PUBLIC DATABASE |");
    let alter_session_ctx = analyze_inline_cursor_sql("ALTER SESSION |");
    let alter_session_set_ctx = analyze_inline_cursor_sql("ALTER SESSION SET |");
    let analyze_ctx = analyze_inline_cursor_sql("ANALYZE |");
    let optimize_ctx = analyze_inline_cursor_sql("OPTIMIZE |");
    let check_ctx = analyze_inline_cursor_sql("CHECK |");
    let repair_ctx = analyze_inline_cursor_sql("REPAIR |");
    let create_synonym_name_ctx = analyze_inline_cursor_sql("CREATE SYNONYM emp_syn |");

    let create_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &create_ctx, None);
    let create_or_replace_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_ctx, None);
    let create_or_replace_materialized_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &create_or_replace_materialized_ctx,
            None,
        );
    let create_or_replace_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_editioning_ctx, None);
    let create_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_editioning_ctx, None);
    let drop_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_ctx, None);
    let drop_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_public_ctx, None);
    let drop_package_body_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("B", &drop_package_body_ctx, None);
    let create_unique_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_unique_ctx, None);
    let create_bitmap_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_bitmap_ctx, None);
    let create_global_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_global_ctx, None);
    let create_global_temporary_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_global_temporary_ctx, None);
    let create_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_public_ctx, None);
    let create_or_replace_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_public_ctx, None);
    let create_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_database_ctx, None);
    let create_or_replace_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_database_ctx, None);
    let create_public_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_public_database_ctx, None);
    let create_shared_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_shared_ctx, None);
    let create_or_replace_shared_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_shared_ctx, None);
    let create_shared_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_shared_database_ctx, None);
    let create_shared_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_shared_public_ctx, None);
    let drop_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_database_ctx, None);
    let drop_public_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_public_database_ctx, None);
    let drop_shared_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_shared_ctx, None);
    let alter_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_ctx, None);
    let alter_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_public_ctx, None);
    let alter_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_database_ctx, None);
    let alter_public_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_public_database_ctx, None);
    let alter_shared_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_shared_ctx, None);
    let alter_shared_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_shared_database_ctx, None);
    let alter_shared_public_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_shared_public_ctx, None);
    let alter_shared_public_database_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_shared_public_database_ctx, None);
    let alter_session_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_session_ctx, None);
    let alter_session_set_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_session_set_ctx, None);
    let analyze_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &analyze_ctx, None);
    let optimize_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &optimize_ctx, None);
    let check_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &check_ctx, None);
    let repair_suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &repair_ctx, None);
    let create_synonym_name_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_synonym_name_ctx, None);

    assert!(create_suggestions.iter().any(|value| value == "EDITIONING"));
    for value in [
        "DIRECTORY",
        "TABLESPACE",
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
    ] {
        assert!(
            create_suggestions.iter().any(|suggestion| suggestion == value),
            "CREATE keyword suggestions should include {value}: {create_suggestions:?}"
        );
    }
    assert!(create_or_replace_suggestions.iter().any(|value| value == "INDEX"));
    assert!(create_or_replace_suggestions
        .iter()
        .any(|value| value == "EDITIONING"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "PACKAGE"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "TRIGGER"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "TYPE"));
    assert!(create_or_replace_suggestions.iter().any(|value| value == "USER"));
    assert!(create_or_replace_suggestions
        .iter()
        .any(|value| value == "DIRECTORY"));
    assert!(create_or_replace_suggestions
        .iter()
        .any(|value| value == "JAVA"));
    assert!(create_or_replace_suggestions
        .iter()
        .any(|value| value == "LIBRARY"));
    assert!(!create_or_replace_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert!(!create_or_replace_suggestions
        .iter()
        .any(|value| value == "SHARED"));
    assert_eq!(
        create_or_replace_materialized_suggestions,
        vec!["VIEW".to_string()]
    );
    assert_eq!(
        create_or_replace_editioning_suggestions,
        vec!["VIEW".to_string()]
    );
    assert_eq!(create_editioning_suggestions, vec!["VIEW".to_string()]);
    for value in [
        "DIRECTORY",
        "TABLESPACE",
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
    ] {
        assert!(
            drop_suggestions.iter().any(|suggestion| suggestion == value),
            "DROP keyword suggestions should include {value}: {drop_suggestions:?}"
        );
    }
    assert!(drop_public_suggestions.iter().any(|value| value == "SYNONYM"));
    assert!(drop_public_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert_eq!(drop_package_body_suggestions, vec!["BODY".to_string()]);
    assert_eq!(create_unique_suggestions, vec!["INDEX".to_string()]);
    assert_eq!(create_bitmap_suggestions, vec!["INDEX".to_string()]);
    assert_eq!(create_global_suggestions, vec!["TEMPORARY".to_string()]);
    assert_eq!(
        create_global_temporary_suggestions,
        vec!["TABLE".to_string()]
    );
    assert!(create_public_suggestions.iter().any(|value| value == "SYNONYM"));
    assert!(create_public_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert!(create_public_suggestions
        .iter()
        .any(|value| value == "ROLLBACK"));
    assert!(create_or_replace_public_suggestions
        .iter()
        .any(|value| value == "SYNONYM"));
    assert!(!create_or_replace_public_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert_eq!(create_database_suggestions, vec!["LINK".to_string()]);
    assert!(create_or_replace_database_suggestions.is_empty());
    assert_eq!(
        create_public_database_suggestions,
        vec!["LINK".to_string()]
    );
    assert!(create_shared_suggestions
        .iter()
        .any(|value| value == "PUBLIC"));
    assert!(create_shared_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert!(create_or_replace_shared_suggestions.is_empty());
    assert_eq!(
        create_shared_database_suggestions,
        vec!["LINK".to_string()]
    );
    assert_eq!(
        create_shared_public_suggestions,
        vec!["DATABASE".to_string()]
    );
    assert_eq!(drop_database_suggestions, vec!["LINK".to_string()]);
    assert_eq!(
        drop_public_database_suggestions,
        vec!["LINK".to_string()]
    );
    assert!(drop_shared_suggestions.is_empty());
    assert!(alter_suggestions.iter().any(|value| value == "SESSION"));
    for value in [
        "TABLESPACE",
        "ROLE",
        "PROFILE",
        "ROLLBACK",
        "JAVA",
        "LIBRARY",
        "CLUSTER",
        "DIMENSION",
        "OPERATOR",
        "INDEXTYPE",
        "SYSTEM",
    ] {
        assert!(
            alter_suggestions.iter().any(|suggestion| suggestion == value),
            "ALTER keyword suggestions should include {value}: {alter_suggestions:?}"
        );
    }
    assert!(
        !alter_suggestions.iter().any(|value| value == "DIRECTORY"),
        "ALTER keyword suggestions should not include DIRECTORY: {alter_suggestions:?}"
    );
    assert!(alter_suggestions.iter().any(|value| value == "PUBLIC"));
    assert!(alter_suggestions.iter().any(|value| value == "DATABASE"));
    assert!(alter_suggestions.iter().any(|value| value == "SHARED"));
    assert!(alter_public_suggestions.iter().any(|value| value == "SYNONYM"));
    assert!(alter_public_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert_eq!(alter_database_suggestions, vec!["LINK".to_string()]);
    assert_eq!(
        alter_public_database_suggestions,
        vec!["LINK".to_string()]
    );
    assert!(alter_shared_suggestions
        .iter()
        .any(|value| value == "PUBLIC"));
    assert!(alter_shared_suggestions
        .iter()
        .any(|value| value == "DATABASE"));
    assert_eq!(
        alter_shared_database_suggestions,
        vec!["LINK".to_string()]
    );
    assert_eq!(
        alter_shared_public_suggestions,
        vec!["DATABASE".to_string()]
    );
    assert_eq!(
        alter_shared_public_database_suggestions,
        vec!["LINK".to_string()]
    );
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
fn collect_expected_keyword_suggestions_complete_plsql_body_object_tails() {
    for sql in [
        "CREATE PACKAGE |",
        "CREATE OR REPLACE PACKAGE |",
        "CREATE TYPE |",
        "CREATE OR REPLACE TYPE |",
        "DROP PACKAGE |",
        "DROP TYPE |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None);
        assert_eq!(
            suggestions,
            vec!["BODY".to_string()],
            "PL/SQL body tail should complete BODY for `{sql}`"
        );
    }

    let drop_package_prefix_ctx = analyze_inline_cursor_sql("DROP PACKAGE B|");
    let drop_package_prefix_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("B", &drop_package_prefix_ctx, None);
    assert_eq!(drop_package_prefix_suggestions, vec!["BODY".to_string()]);

    for sql in ["ALTER PACKAGE |", "ALTER TYPE |"] {
        let ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None);
        assert!(
            !suggestions.iter().any(|value| value == "BODY"),
            "ALTER object-name context should not suggest BODY before the object name for `{sql}`"
        );
    }
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
            (
                "DATA_PUMP_DIR".to_string(),
                Some(QualifiedMemberKind::Directory),
            ),
            ("APP_LINK".to_string(), Some(QualifiedMemberKind::DatabaseLink)),
            ("Welcome".to_string(), Some(QualifiedMemberKind::JavaSource)),
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
            "APP_LINK".to_string(),
            "DATA_PUMP_DIR".to_string(),
            "EMP".to_string(),
            "EMP_API".to_string(),
            "EMP_SEQ".to_string(),
            "EMP_T".to_string(),
            "EMP_VIEW".to_string(),
            "Welcome".to_string(),
        ]
    );
}

#[test]
fn collect_expected_object_suggestions_filter_by_object_type_and_include_users() {
    let drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE |");
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE BODY |");
    let drop_type_ctx = analyze_inline_cursor_sql("DROP TYPE |");
    let drop_type_body_ctx = analyze_inline_cursor_sql("DROP TYPE BODY |");
    let drop_trigger_ctx = analyze_inline_cursor_sql("DROP TRIGGER |");
    let drop_index_ctx = analyze_inline_cursor_sql("DROP INDEX |");
    let alter_synonym_ctx = analyze_inline_cursor_sql("ALTER SYNONYM |");
    let alter_public_synonym_ctx = analyze_inline_cursor_sql("ALTER PUBLIC SYNONYM |");
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
    data.synonyms = vec!["EMP_SYN".to_string()];
    data.public_synonyms = vec!["EMP_PUBLIC_SYN".to_string()];
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
    let type_body_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_type_body_ctx);
    let trigger_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_trigger_ctx);
    let index_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &drop_index_ctx);
    let alter_synonym_suggestions =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &alter_synonym_ctx);
    let alter_public_synonym_suggestions = SqlEditorWidget::collect_expected_object_suggestions(
        &mut data,
        "",
        &alter_public_synonym_ctx,
    );
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
    assert_eq!(type_body_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(trigger_suggestions, vec!["EMP_BIU_TRG".to_string()]);
    assert_eq!(index_suggestions, vec!["EMP_PK".to_string()]);
    assert_eq!(alter_synonym_suggestions, vec!["EMP_SYN".to_string()]);
    assert_eq!(
        alter_public_synonym_suggestions,
        vec!["EMP_PUBLIC_SYN".to_string()]
    );
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
fn collect_expected_object_suggestions_filter_extended_oracle_object_types() {
    let mut data = IntellisenseData::new();
    data.database_links = vec!["APP_LINK".to_string()];
    data.directories = vec!["DATA_PUMP_DIR".to_string()];
    data.libraries = vec!["APP_LIB".to_string()];
    data.clusters = vec!["EMP_CLUSTER".to_string()];
    data.contexts = vec!["APP_CTX".to_string()];
    data.dimensions = vec!["SALES_DIM".to_string()];
    data.operators = vec!["EQ_OP".to_string()];
    data.indextypes = vec!["TEXT_ITYPE".to_string()];
    data.editions = vec!["V2_EDITION".to_string()];
    data.java_sources = vec!["Welcome".to_string()];
    data.java_classes = vec!["Agent".to_string()];
    data.java_resources = vec!["appText".to_string()];
    data.tables = vec!["EMP".to_string()];
    data.rebuild_indices();

    for (sql, expected) in [
        ("DROP DATABASE LINK |", "APP_LINK"),
        ("DROP PUBLIC DATABASE LINK |", "APP_LINK"),
        ("ALTER DATABASE LINK |", "APP_LINK"),
        ("ALTER PUBLIC DATABASE LINK |", "APP_LINK"),
        ("DROP DIRECTORY |", "DATA_PUMP_DIR"),
        ("ALTER LIBRARY |", "APP_LIB"),
        ("DROP LIBRARY |", "APP_LIB"),
        ("ALTER CLUSTER |", "EMP_CLUSTER"),
        ("DROP CLUSTER |", "EMP_CLUSTER"),
        ("DROP CONTEXT |", "APP_CTX"),
        ("ALTER DIMENSION |", "SALES_DIM"),
        ("DROP DIMENSION |", "SALES_DIM"),
        ("ALTER OPERATOR |", "EQ_OP"),
        ("DROP OPERATOR |", "EQ_OP"),
        ("ALTER INDEXTYPE |", "TEXT_ITYPE"),
        ("DROP INDEXTYPE |", "TEXT_ITYPE"),
        ("DROP EDITION |", "V2_EDITION"),
        ("ALTER JAVA SOURCE |", "Welcome"),
        ("DROP JAVA SOURCE |", "Welcome"),
        ("ALTER JAVA CLASS |", "Agent"),
        ("DROP JAVA CLASS |", "Agent"),
        ("DROP JAVA RESOURCE |", "appText"),
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &ctx);
        assert_eq!(
            suggestions,
            vec![expected.to_string()],
            "object suggestions should filter to `{expected}` for `{sql}`"
        );
        assert!(
            !suggestions.iter().any(|value| value == "EMP"),
            "object suggestions for `{sql}` should not include unrelated tables: {suggestions:?}"
        );
    }
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
        "CREATE MATERIALIZED VIEW LOG ON |",
        "ALTER MATERIALIZED VIEW LOG ON |",
        "DROP MATERIALIZED VIEW LOG ON |",
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
    let create_mv_log_ctx = analyze_inline_cursor_sql("CREATE MATERIALIZED VIEW LOG ON scott.|");
    let alter_mv_log_ctx = analyze_inline_cursor_sql("ALTER MATERIALIZED VIEW LOG ON scott.|");
    let drop_mv_log_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED VIEW LOG ON scott.|");
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
    let create_mv_log_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &create_mv_log_ctx,
        );
    let alter_mv_log_suggestions =
        SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
            &mut data,
            "scott",
            "",
            &alter_mv_log_ctx,
        );
    let drop_mv_log_suggestions = SqlEditorWidget::expected_relation_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_mv_log_ctx,
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
    assert_eq!(create_mv_log_suggestions, vec!["EMP".to_string()]);
    assert_eq!(alter_mv_log_suggestions, vec!["EMP".to_string()]);
    assert_eq!(drop_mv_log_suggestions, vec!["EMP".to_string()]);
    assert_eq!(mv_suggestions, vec!["EMP_MV".to_string()]);
    assert_eq!(comment_table_suggestions, vec!["EMP".to_string()]);
    assert_eq!(comment_view_suggestions, vec!["EMP_VIEW".to_string()]);
    assert_eq!(comment_editioning_view_suggestions, vec!["EMP_VIEW".to_string()]);
}

#[test]
fn schema_object_member_suggestions_cover_oracle_ddl_object_types() {
    let drop_package_ctx = analyze_inline_cursor_sql("DROP PACKAGE scott.|");
    let drop_package_body_ctx = analyze_inline_cursor_sql("DROP PACKAGE BODY scott.|");
    let drop_type_ctx = analyze_inline_cursor_sql("DROP TYPE scott.|");
    let drop_type_body_ctx = analyze_inline_cursor_sql("DROP TYPE BODY scott.|");
    let alter_trigger_ctx = analyze_inline_cursor_sql("ALTER TRIGGER scott.|");
    let alter_synonym_ctx = analyze_inline_cursor_sql("ALTER SYNONYM scott.|");
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
            ("EMP_SYN".to_string(), Some(QualifiedMemberKind::Synonym)),
        ],
    );

    let package_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &drop_package_ctx,
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
    let synonym_suggestions = SqlEditorWidget::expected_member_suggestions_for_qualifier(
        &mut data,
        "scott",
        "",
        &alter_synonym_ctx,
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

    assert_eq!(package_suggestions, vec!["UTIL_PKG".to_string()]);
    assert_eq!(package_body_suggestions, vec!["UTIL_PKG".to_string()]);
    assert_eq!(type_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(type_body_suggestions, vec!["ADDRESS_T".to_string()]);
    assert_eq!(trigger_suggestions, vec!["EMP_BIU_TRG".to_string()]);
    assert_eq!(synonym_suggestions, vec!["EMP_SYN".to_string()]);
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
            "EMP_SYN".to_string(),
            "SALES_MV".to_string(),
            "SEQ_ORDER".to_string()
        ]
    );
    assert_eq!(
        grant_multi_relation_suggestions,
        vec![
            "EMP".to_string(),
            "EMP_SYN".to_string(),
            "SALES_MV".to_string(),
            "SEQ_ORDER".to_string()
        ]
    );
}

#[test]
fn collect_expected_keyword_suggestions_complete_materialized_view_tail() {
    let drop_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED |");
    let create_mv_ctx = analyze_inline_cursor_sql("CREATE MATERIALIZED VIEW |");
    let create_mv_log_ctx = analyze_inline_cursor_sql("CREATE MATERIALIZED VIEW LOG |");
    let alter_mv_ctx = analyze_inline_cursor_sql("ALTER MATERIALIZED VIEW |");
    let alter_mv_log_ctx = analyze_inline_cursor_sql("ALTER MATERIALIZED VIEW LOG |");
    let drop_mv_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED VIEW |");
    let drop_mv_log_ctx = analyze_inline_cursor_sql("DROP MATERIALIZED VIEW LOG |");
    let comment_on_ctx = analyze_inline_cursor_sql("COMMENT ON |");
    let comment_editioning_ctx = analyze_inline_cursor_sql("COMMENT ON EDITIONING |");
    let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_ctx, None);
    let create_mv_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_mv_ctx, None);
    let create_mv_log_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_mv_log_ctx, None);
    let alter_mv_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_mv_ctx, None);
    let alter_mv_log_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_mv_log_ctx, None);
    let drop_mv_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_mv_ctx, None);
    let drop_mv_log_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_mv_log_ctx, None);
    let comment_on_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &comment_on_ctx, None);
    let comment_editioning_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &comment_editioning_ctx, None);

    assert_eq!(suggestions, vec!["VIEW".to_string()]);
    assert_eq!(create_mv_suggestions, vec!["LOG".to_string()]);
    assert_eq!(create_mv_log_suggestions, vec!["ON".to_string()]);
    assert_eq!(alter_mv_suggestions, vec!["LOG".to_string()]);
    assert_eq!(alter_mv_log_suggestions, vec!["ON".to_string()]);
    assert_eq!(drop_mv_suggestions, vec!["LOG".to_string()]);
    assert_eq!(drop_mv_log_suggestions, vec!["ON".to_string()]);
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
fn collect_expected_keyword_suggestions_complete_rollback_and_java_tails() {
    for sql in [
        "CREATE ROLLBACK |",
        "CREATE PUBLIC ROLLBACK |",
        "ALTER ROLLBACK |",
        "DROP ROLLBACK |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None);
        assert_eq!(
            suggestions,
            vec!["SEGMENT".to_string()],
            "ROLLBACK tail should complete SEGMENT for `{sql}`"
        );
    }

    let create_java_ctx = analyze_inline_cursor_sql("CREATE JAVA |");
    let create_or_replace_java_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE JAVA |");
    let create_or_replace_and_ctx = analyze_inline_cursor_sql("CREATE OR REPLACE AND |");
    let create_or_replace_and_compile_ctx =
        analyze_inline_cursor_sql("CREATE OR REPLACE AND COMPILE |");
    let create_or_replace_and_resolve_ctx =
        analyze_inline_cursor_sql("CREATE OR REPLACE AND RESOLVE |");
    let create_or_replace_and_compile_java_ctx =
        analyze_inline_cursor_sql("CREATE OR REPLACE AND COMPILE JAVA |");
    let alter_java_ctx = analyze_inline_cursor_sql("ALTER JAVA |");
    let drop_java_ctx = analyze_inline_cursor_sql("DROP JAVA |");
    let create_java_source_ctx = analyze_inline_cursor_sql("CREATE JAVA SOURCE |");
    let create_java_resource_ctx = analyze_inline_cursor_sql("CREATE JAVA RESOURCE |");
    let create_java_class_ctx = analyze_inline_cursor_sql("CREATE JAVA CLASS |");

    let create_java_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_java_ctx, None);
    let create_or_replace_java_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_java_ctx, None);
    let create_or_replace_and_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_or_replace_and_ctx, None);
    let create_or_replace_and_compile_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &create_or_replace_and_compile_ctx,
            None,
        );
    let create_or_replace_and_resolve_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &create_or_replace_and_resolve_ctx,
            None,
        );
    let create_or_replace_and_compile_java_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &create_or_replace_and_compile_java_ctx,
            None,
        );
    let alter_java_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &alter_java_ctx, None);
    let drop_java_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &drop_java_ctx, None);
    let create_java_source_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_java_source_ctx, None);
    let create_java_resource_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_java_resource_ctx, None);
    let create_java_class_suggestions =
        SqlEditorWidget::collect_expected_keyword_suggestions("", &create_java_class_ctx, None);

    assert_eq!(
        create_java_suggestions,
        vec![
            "SOURCE".to_string(),
            "CLASS".to_string(),
            "RESOURCE".to_string()
        ]
    );
    assert_eq!(create_or_replace_java_suggestions, create_java_suggestions);
    assert_eq!(
        create_or_replace_and_suggestions,
        vec!["COMPILE".to_string(), "RESOLVE".to_string()]
    );
    assert_eq!(
        create_or_replace_and_compile_suggestions,
        vec!["JAVA".to_string()]
    );
    assert_eq!(
        create_or_replace_and_resolve_suggestions,
        vec!["JAVA".to_string()]
    );
    assert_eq!(
        create_or_replace_and_compile_java_suggestions,
        create_java_suggestions
    );
    assert_eq!(
        alter_java_suggestions,
        vec!["SOURCE".to_string(), "CLASS".to_string()]
    );
    assert_eq!(drop_java_suggestions, create_java_suggestions);
    assert_eq!(create_java_source_suggestions, vec!["NAMED".to_string()]);
    assert_eq!(create_java_resource_suggestions, vec!["NAMED".to_string()]);
    assert_eq!(create_java_class_suggestions, vec!["USING".to_string()]);
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

    let emp_id = descriptions.get("EMP_ID").expect("EMP_ID detail");
    assert_eq!(emp_id.type_text, "NUMBER(10)");
    assert_eq!(emp_id.badges, "PK");
    let ename = descriptions.get("ENAME").expect("ENAME detail");
    assert_eq!(ename.type_text, "VARCHAR2(50)");
    assert_eq!(ename.badges, "");
    // Non-PK NOT NULL column that is also a foreign key shows both badges.
    let deptno = descriptions.get("DEPTNO").expect("DEPTNO detail");
    assert_eq!(deptno.type_text, "NUMBER");
    assert_eq!(deptno.badges, "NN  FK\u{2192}DEPT");
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

    let dept_no = descriptions.get("DEPT NO").expect("DEPT NO detail");
    assert_eq!(dept_no.type_text, "NUMBER");
    assert_eq!(dept_no.badges, "NN  FK\u{2192}DEPT");
    let emp_name = descriptions.get("EMP NAME").expect("EMP NAME detail");
    assert_eq!(emp_name.type_text, "VARCHAR2(50)");
    assert_eq!(emp_name.badges, "");
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

/// Resolve window-frame keyword candidates at the `|` marker.
fn window_frame_candidates(sql_with_cursor: &str) -> Option<Vec<String>> {
    let cursor = sql_with_cursor
        .find('|')
        .expect("cursor marker should exist");
    let sql = sql_with_cursor.replace('|', "");
    let token_spans = super::query_text::tokenize_sql_spanned(&sql);
    let split_idx = token_spans.partition_point(|span| span.end <= cursor);
    let tokens: Vec<SqlToken> = token_spans.into_iter().map(|span| span.token).collect();
    SqlEditorWidget::expected_window_frame_keyword_candidates(&tokens, split_idx)
        .map(|c| c.iter().map(|s| s.to_string()).collect())
}

#[test]
fn window_frame_after_unit_suggests_between_and_bounds() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d ROWS |) FROM t"),
        Some(vec!["BETWEEN".into(), "UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_after_between_suggests_first_bound() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN |) FROM t"),
        Some(vec!["UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_after_unbounded_suggests_direction() {
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED |) FROM t"
        ),
        Some(vec!["PRECEDING".into(), "FOLLOWING".into()])
    );
}

#[test]
fn window_frame_after_current_suggests_row() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d RANGE CURRENT |) FROM t"),
        Some(vec!["ROW".into()])
    );
}

#[test]
fn window_frame_after_and_suggests_second_bound() {
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING AND |) FROM t"
        ),
        Some(vec!["UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_after_first_bound_suggests_and() {
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING |) FROM t"
        ),
        Some(vec!["AND".into()])
    );
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN 5 PRECEDING |) FROM t"
        ),
        Some(vec!["AND".into()])
    );
}

#[test]
fn window_frame_after_complete_bound_suggests_exclude() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW |) FROM t"),
        Some(vec!["EXCLUDE".into()])
    );
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW |) FROM t"
        ),
        Some(vec!["EXCLUDE".into()])
    );
}

#[test]
fn window_frame_exclude_tail_suggests_only_fixed_keywords() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE |) FROM t"),
        Some(vec![
            "CURRENT".into(),
            "GROUP".into(),
            "TIES".into(),
            "NO".into()
        ])
    );
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE CURRENT |) FROM t"
        ),
        Some(vec!["ROW".into()])
    );
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE NO |) FROM t"
        ),
        Some(vec!["OTHERS".into()])
    );
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE NO OTHERS |) FROM t"
        ),
        Some(Vec::new())
    );
}

#[test]
fn window_frame_groups_unit_is_recognized() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY d GROUPS |) FROM t"),
        Some(vec!["BETWEEN".into(), "UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_keyword_only_positions_suppress_columns() {
    let at = |sql: &str| {
        SqlEditorWidget::cursor_is_at_window_frame_keyword_only_position_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    // Fixed frame keyword tails only accept frame keywords, so columns are
    // suppressed.
    assert!(at("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED |) FROM t"));
    assert!(at("SELECT sum(x) OVER (ORDER BY d RANGE CURRENT |) FROM t"));
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED |) FROM t"
    ));
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING |) FROM t"
    ));
    assert!(at("SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW |) FROM t"));
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE |) FROM t"
    ));
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE CURRENT |) FROM t"
    ));
    assert!(at(
        "SELECT sum(x) OVER (ORDER BY d ROWS CURRENT ROW EXCLUDE NO OTHERS |) FROM t"
    ));
    // Value-accepting frame slots keep columns visible (a bound may be an
    // expression, e.g. `ROWS 5 PRECEDING` / `RANGE BETWEEN x PRECEDING`).
    assert!(!at("SELECT sum(x) OVER (ORDER BY d ROWS |) FROM t"));
    assert!(!at("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN |) FROM t"));
    assert!(!at("SELECT sum(x) OVER (ORDER BY d ROWS BETWEEN UNBOUNDED PRECEDING AND |) FROM t"));
    // A column named `unbounded`/`current` outside any window spec, or inside the
    // window ORDER BY before a frame unit, is unaffected.
    assert!(!at("SELECT unbounded | FROM t"));
    assert!(!at("SELECT current | FROM t"));
    assert!(!at("SELECT sum(x) OVER (ORDER BY current |) FROM t"));
    assert!(!at("SELECT sum(x) OVER (ORDER BY unbounded |) FROM t"));
}

#[test]
fn ordinary_between_predicate_is_not_treated_as_window_frame() {
    // No OVER -> a normal range predicate must never offer frame keywords.
    assert_eq!(
        window_frame_candidates("SELECT * FROM t WHERE x BETWEEN 1 AND |"),
        None
    );
}

#[test]
fn column_named_range_outside_window_does_not_trigger_frame() {
    assert_eq!(
        window_frame_candidates("SELECT t.range | FROM t"),
        None
    );
}

#[test]
fn case_expression_and_inside_over_is_not_a_frame_bound() {
    // AND inside OVER but with no frame marker (a boolean in ORDER BY CASE) must
    // not be mistaken for a frame second-bound position.
    assert_eq!(
        window_frame_candidates(
            "SELECT sum(x) OVER (ORDER BY CASE WHEN a AND |) FROM t"
        ),
        None
    );
}

#[test]
fn window_frame_unit_after_closed_over_does_not_trigger() {
    // `rows` is an ordinary identifier after a *closed* OVER(); the cursor is
    // not inside any window paren, so no frame keywords.
    assert_eq!(
        window_frame_candidates("SELECT count(*) OVER () r, x ROWS | FROM t"),
        None
    );
}

#[test]
fn window_frame_current_after_closed_over_does_not_trigger() {
    assert_eq!(
        window_frame_candidates("SELECT count(*) OVER (), current | FROM t"),
        None
    );
}

#[test]
fn window_frame_keywords_require_a_frame_unit() {
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY current |) FROM t"),
        None
    );
    assert_eq!(
        window_frame_candidates("SELECT sum(x) OVER (ORDER BY unbounded |) FROM t"),
        None
    );
}

#[test]
fn window_frame_numeric_first_bound_then_and_suggests_second_bound() {
    assert_eq!(
        window_frame_candidates(
            "SELECT avg(x) OVER (ORDER BY d ROWS BETWEEN 5 PRECEDING AND |) FROM t"
        ),
        Some(vec!["UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_inside_subquery_over_is_recognized() {
    assert_eq!(
        window_frame_candidates(
            "SELECT * FROM (SELECT sum(x) OVER (ORDER BY d ROWS |) FROM t) z"
        ),
        Some(vec!["BETWEEN".into(), "UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_prefix_filters_candidates_through_production_path() {
    // Faithful to runtime wiring: the partial word is trimmed by
    // `expected_suggestion_context_end`, leaving the cursor at the `ROWS`
    // position; the `UNB` prefix then narrows the frame candidates.
    let ctx = analyze_inline_cursor_sql("SELECT avg(x) OVER (ORDER BY d ROWS UNB|) FROM t");
    let suggestions = SqlEditorWidget::collect_expected_keyword_suggestions("UNB", &ctx, None);
    assert_eq!(suggestions, vec!["UNBOUNDED".to_string()]);
}

#[test]
fn window_frame_inside_named_window_clause_is_recognized() {
    assert_eq!(
        window_frame_candidates(
            "SELECT count(*) OVER w FROM t WINDOW w AS (ORDER BY d ROWS |)"
        ),
        Some(vec!["BETWEEN".into(), "UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn window_frame_inside_named_window_clause_in_subquery_is_recognized() {
    assert_eq!(
        window_frame_candidates(
            "SELECT * FROM (SELECT count(*) OVER w FROM t WINDOW w AS (ORDER BY d ROWS BETWEEN |)) z"
        ),
        Some(vec!["UNBOUNDED".into(), "CURRENT".into()])
    );
}

#[test]
fn cte_body_as_paren_is_not_a_window_spec() {
    // `WITH c AS (...)` shares the `name AS (` shape with a named window but must
    // never offer frame keywords for a column called `rows`.
    assert_eq!(
        window_frame_candidates("WITH c AS (SELECT x rows | FROM t) SELECT * FROM c"),
        None
    );
}

/// Data-type keyword suggestions at the `|` marker for a given dialect.
fn data_type_suggestions(
    sql_with_cursor: &str,
    prefix: &str,
    db: crate::db::DatabaseType,
) -> Vec<String> {
    let ctx = analyze_inline_cursor_sql(sql_with_cursor);
    SqlEditorWidget::collect_expected_keyword_suggestions(prefix, &ctx, Some(db))
}

#[test]
fn data_type_cast_as_offers_oracle_types() {
    let s = data_type_suggestions("SELECT CAST(x AS |) FROM t", "", crate::db::DatabaseType::Oracle);
    assert!(s.contains(&"VARCHAR2".to_string()));
    assert!(s.contains(&"NUMBER".to_string()));
    assert!(s.contains(&"TIMESTAMP".to_string()));
}

#[test]
fn data_type_cast_as_prefix_filters() {
    assert_eq!(
        data_type_suggestions("SELECT CAST(x AS NUMB|) FROM t", "NUMB", crate::db::DatabaseType::Oracle),
        vec!["NUMBER".to_string()]
    );
}

#[test]
fn data_type_treat_as_is_a_type_position() {
    let s = data_type_suggestions("SELECT TREAT(x AS |) FROM t", "", crate::db::DatabaseType::Oracle);
    assert!(s.contains(&"XMLTYPE".to_string()));
}

#[test]
fn data_type_xmlserialize_and_validate_conversion_as_are_type_positions() {
    // XMLSERIALIZE and VALIDATE_CONVERSION share CAST's `AS <type>` slot.
    let xs = data_type_suggestions(
        "SELECT XMLSERIALIZE(DOCUMENT x AS |) FROM t",
        "",
        crate::db::DatabaseType::Oracle,
    );
    assert!(xs.contains(&"CLOB".to_string()));
    let vc = data_type_suggestions(
        "SELECT VALIDATE_CONVERSION(x AS |) FROM t",
        "",
        crate::db::DatabaseType::Oracle,
    );
    assert!(vc.contains(&"NUMBER".to_string()));
    // The expression before AS is still a normal column position.
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
        &analyze_inline_cursor_sql("SELECT XMLSERIALIZE(DOCUMENT | AS CLOB) FROM t"),
        false,
    ));
}

#[test]
fn data_type_mysql_cast_uses_restricted_grammar() {
    let s = data_type_suggestions("SELECT CAST(x AS |) FROM t", "", crate::db::DatabaseType::MySQL);
    assert!(s.contains(&"SIGNED".to_string()));
    assert!(s.contains(&"UNSIGNED".to_string()));
    // VARCHAR is not valid in a MySQL CAST.
    assert!(!s.contains(&"VARCHAR".to_string()));
}

#[test]
fn data_type_precision_argument_is_not_a_type_position() {
    assert!(
        data_type_suggestions("SELECT CAST(x AS NUMBER(|)) FROM t", "", crate::db::DatabaseType::Oracle)
            .is_empty()
    );
}

#[test]
fn keyword_only_slots_suppress_the_identifier_base() {
    // `trigger_intellisense` keys the identifier-base branch on these two
    // predicates: a data-type slot keeps only user TYPE objects, every other
    // column-suppressing keyword/value slot keeps nothing. Guard both so the
    // base can never silently start leaking relations/functions/columns again.
    // `exclude_current_identifier_chain` mirrors the production wiring
    // (`!prefix.is_empty()`), derived here from the marked word at the cursor.
    fn exclude_flag(sql_with_cursor: &str) -> bool {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        !prefix.is_empty()
    }

    let type_slots = [
        "SELECT CAST(x AS |) FROM dual",
        "SELECT CAST(x AS emp|) FROM dual",
        "CREATE TABLE t (c |)",
    ];
    for sql in type_slots {
        let ctx = analyze_inline_cursor_sql(sql);
        let exclude = exclude_flag(sql);
        assert!(
            SqlEditorWidget::data_type_position_for_context(&ctx, exclude).is_some(),
            "expected a data-type position for `{sql}`"
        );
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, exclude),
            "data-type position must be a column-suppressing slot for `{sql}`"
        );
    }

    // Non-data-type keyword/value-only slots: column-suppressing, and crucially
    // NOT a data-type position (so the base goes empty rather than to types).
    let pure_keyword_slots = [
        "SELECT EXTRACT(yea| FROM d) FROM dual",
        "SELECT INTERVAL '1' yea| FROM dual",
        "SELECT SUM(x) OVER (ORDER BY a ROWS UNBOUNDED prec|) FROM t",
        "SELECT x FROM t FETCH FIRST ro| ROWS ONLY",
        "SELECT max(x) KEEP (DENSE_RANK fir|) FROM t",
    ];
    for sql in pure_keyword_slots {
        let ctx = analyze_inline_cursor_sql(sql);
        let exclude = exclude_flag(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, exclude),
            "expected a column-suppressing slot for `{sql}`"
        );
        assert!(
            SqlEditorWidget::data_type_position_for_context(&ctx, exclude).is_none(),
            "`{sql}` must not be treated as a data-type position"
        );
    }

    // A plain column position is neither, so its identifier base is unaffected.
    let plain = analyze_inline_cursor_sql("SELECT ename, | FROM emp");
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&plain, false));
    assert!(SqlEditorWidget::data_type_position_for_context(&plain, false).is_none());
}

/// `GRANT`/`REVOKE` privilege lists reuse DML keywords (`SELECT`, `INSERT`,
/// `UPDATE`, `DELETE`, …) that previously flipped the cursor into a query/DML
/// phase, so the object slot (`… ON <object>`) wrongly offered columns/`*`
/// instead of relations, and the grantee slot (`… TO|FROM <user>`) offered
/// columns/tables. The privilege keyword must never put the cursor in a
/// column or table phase anywhere in a `GRANT`/`REVOKE` statement.
#[test]
fn grant_revoke_privilege_keywords_never_enter_column_or_table_phase() {
    let object_slots = [
        "GRANT SELECT ON | TO u",
        "GRANT SELECT, UPDATE ON | TO u",
        "REVOKE SELECT ON | FROM u",
        "REVOKE DELETE ON | FROM u",
        "GRANT INSERT ON | TO u",
        "GRANT UPDATE ON | TO u",
    ];
    for sql in object_slots {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );
        assert_eq!(
            context,
            SqlContext::General,
            "object slot should not be a column/table context: {sql}"
        );
        // The object name is still surfaced via the expected-object machinery.
        assert!(
            SqlEditorWidget::expected_object_suggestion_kind("", None, &deep_ctx).is_some(),
            "object slot should expect an object kind: {sql}"
        );
    }

    // Grantee slots name a user, not a relation/column: neither columns nor
    // tables may be offered there.
    for sql in ["GRANT SELECT ON t TO |", "REVOKE SELECT ON t FROM |"] {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );
        assert_eq!(
            context,
            SqlContext::General,
            "grantee slot should not be a column/table context: {sql}"
        );
    }
}

/// The object slot of a `GRANT SELECT`/`REVOKE SELECT` resolves to relations,
/// not the columns of an in-scope identifier. Exercises the apply-path gating
/// end to end: a column context would emit `get_suggestions_for_db` columns.
#[test]
fn grant_select_object_slot_offers_relations_not_columns() {
    let deep_ctx = analyze_inline_cursor_sql("GRANT SELECT ON | TO scott");
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.set_columns_for_table("EMP", vec!["EMPNO".to_string(), "ENAME".to_string()]);
    data.rebuild_indices();

    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert!(!matches!(
        context,
        SqlContext::ColumnName | SqlContext::ColumnOrAll
    ));

    let objects =
        SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &deep_ctx);
    assert!(objects.iter().any(|value| value == "EMP"));
    assert!(
        !objects.iter().any(|value| value == "EMPNO" || value == "ENAME"),
        "columns must not leak into the GRANT object slot: {:?}",
        objects
    );
}

/// An identifier literally named `grant`/`revoke` inside a query keeps normal
/// completion — the statement-head guard must not misfire mid-statement.
#[test]
fn grant_revoke_guard_does_not_affect_identifier_named_grant() {
    let deep_ctx = analyze_inline_cursor_sql("SELECT grant, revoke FROM t WHERE |");
    let context = SqlEditorWidget::classify_intellisense_context(
        &deep_ctx,
        deep_ctx.statement_tokens.as_ref(),
    );
    assert_eq!(context, SqlContext::ColumnName);
}

/// `AUDIT`/`NOAUDIT` belong to the same object-privilege family as
/// `GRANT`/`REVOKE`: their option list reuses DML keywords, so the object slot
/// (`… ON <object>`) must offer objects, not the columns/`*` of a query phase.
#[test]
fn audit_noaudit_object_slot_offers_objects_not_columns() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.set_columns_for_table("EMP", vec!["EMPNO".to_string()]);
    data.rebuild_indices();

    for sql in [
        "AUDIT SELECT ON | BY ACCESS",
        "AUDIT SELECT, UPDATE ON |",
        "NOAUDIT SELECT ON |",
    ] {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );
        assert_eq!(context, SqlContext::General, "{sql}");

        let objects =
            SqlEditorWidget::collect_expected_object_suggestions(&mut data, "", &deep_ctx);
        assert!(objects.iter().any(|value| value == "EMP"), "{sql}: {:?}", objects);
        assert!(
            !objects.iter().any(|value| value == "EMPNO"),
            "columns must not leak into the AUDIT object slot: {sql}: {:?}",
            objects
        );
    }
}

/// A real query embedded after a non-query head (`EXPLAIN PLAN FOR SELECT`,
/// `CREATE TABLE … AS SELECT`, `INSERT … SELECT`) must still reach its select
/// list — the privilege-statement guard only fires for the privilege verbs.
#[test]
fn embedded_query_select_lists_stay_column_contexts() {
    for sql in [
        "EXPLAIN PLAN FOR SELECT | FROM t",
        "CREATE TABLE x AS SELECT | FROM t",
        "INSERT INTO t SELECT | FROM s",
    ] {
        let deep_ctx = analyze_inline_cursor_sql(sql);
        assert_eq!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::SelectList,
            "{sql}"
        );
    }
}

/// Resolves the column scope and base suggestions exactly as the apply path
/// does for a qualified position: `column_tables` empty ⇒ `column_scope` None.
fn qualified_base_suggestions_for(
    data: &mut IntellisenseData,
    sql_with_cursor: &str,
    qualifier: &str,
) -> Vec<String> {
    let ctx = analyze_inline_cursor_sql(sql_with_cursor);
    let column_tables =
        SqlEditorWidget::resolve_column_tables_for_context(Some(qualifier), &ctx);
    let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
    let context =
        SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
    SqlEditorWidget::base_suggestions_for_context(
        data,
        "",
        Some(qualifier),
        column_scope.as_deref(),
        true,
        context,
        ClauseCompletionPolicy::for_phase(ctx.phase, true).restrict_to_relation_columns,
        None,
        ExpressionKeywordContext::ambiguous(),
    )
}

/// A qualified reference must never fall back to the global all-columns list.
/// In a DML target column list (MERGE INSERT/UPDATE SET, INSERT column list) a
/// qualifier that resolves to an in-scope relation *outside* the focused target
/// used to yield an empty scope, which `get_column_suggestions` expands to every
/// column of every table — an unrelated-item dump. Such positions must suggest
/// nothing, while ordinary qualified references still resolve their columns.
#[test]
fn qualified_position_never_dumps_all_columns() {
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string(), "DEPT".to_string()];
    data.set_columns_for_table(
        "EMP",
        vec!["EMPNO".to_string(), "ENAME".to_string(), "DEPTNO".to_string()],
    );
    data.set_columns_for_table("DEPT", vec!["DEPTNO".to_string(), "DNAME".to_string()]);
    data.rebuild_indices();

    // Legitimate qualified references still resolve to their relation's columns.
    let e_cols = qualified_base_suggestions_for(&mut data, "SELECT e.| FROM emp e", "e");
    assert!(e_cols.iter().any(|c| c == "ENAME"));
    assert!(!e_cols.iter().any(|c| c == "DNAME"), "{:?}", e_cols);

    let d_cols = qualified_base_suggestions_for(
        &mut data,
        "SELECT d.| FROM emp e JOIN dept d ON e.deptno = d.deptno",
        "d",
    );
    assert!(d_cols.iter().any(|c| c == "DNAME"));
    assert!(!d_cols.iter().any(|c| c == "ENAME"), "{:?}", d_cols);

    // Unknown qualifier: nothing (not every column).
    assert!(qualified_base_suggestions_for(&mut data, "SELECT zzz.| FROM emp e", "zzz").is_empty());

    // The regression: a cross-scope qualifier inside a focused DML target list
    // must not dump the whole catalog.
    for sql in [
        "MERGE INTO emp e USING dept d ON (e.deptno = d.deptno) WHEN NOT MATCHED THEN INSERT (d.|)",
        "MERGE INTO emp e USING dept d ON (e.deptno = d.deptno) WHEN MATCHED THEN UPDATE SET d.|",
        "INSERT INTO emp (d.|)",
    ] {
        let cols = qualified_base_suggestions_for(&mut data, sql, "d");
        assert!(
            cols.is_empty(),
            "qualified DML-target slot must not dump all columns: {sql}: {:?}",
            cols
        );
    }
}

#[test]
fn select_list_as_alias_slot_suppresses_completion() {
    // After `AS` in a SELECT list the slot names a brand-new column alias, so
    // identifier suggestions are suppressed — both at the empty slot and while
    // the alias is being typed.
    for sql in [
        "SELECT col AS | FROM t",
        "SELECT col AS my| FROM t",
        "SELECT a, b AS | FROM t",
        "SELECT (SELECT 1 FROM dual) AS | FROM t",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        let has_prefix = sql
            .split_once('|')
            .map(|(b, _)| b.ends_with(|ch: char| ch.is_alphanumeric()))
            .unwrap_or(false);
        assert!(
            SqlEditorWidget::cursor_is_at_select_list_alias_name_slot(&ctx, has_prefix),
            "alias slot should suppress: {sql}"
        );
    }

    // Positions where `AS` introduces a type, or where the slot is a real
    // column reference, keep completion.
    for sql in [
        "SELECT CAST(x AS |) FROM t",      // data-type slot
        "SELECT x | FROM t",               // implicit-alias slot (ambiguous)
        "SELECT a AS x, b | FROM t",       // new column reference after comma
        "SELECT * FROM t WHERE col AS |",  // not a select-list context
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        let has_prefix = sql
            .split_once('|')
            .map(|(b, _)| b.ends_with(|ch: char| ch.is_alphanumeric()))
            .unwrap_or(false);
        assert!(
            !SqlEditorWidget::cursor_is_at_select_list_alias_name_slot(&ctx, has_prefix),
            "must not suppress: {sql}"
        );
    }
}

/// The table-clause alias-name slot after `AS` (`FROM t AS |`) names a
/// brand-new relation alias, so the identifier base must be suppressed there —
/// the empty-slot companion of the typed-alias suppression `LocalAliasContext`
/// already applies (`FROM t AS x|`). Previously only the select-list `AS` slot
/// was covered, so `FROM t AS |` leaked the whole relation catalog. Suppression
/// must hold in every table clause (FROM/UPDATE/DELETE/MERGE/JOIN target), and
/// the slot must NOT fire where `AS` introduces a type (`CAST(x AS |)`) or
/// after the flashback `OF` keyword (`FROM t AS OF |`).
#[test]
fn table_alias_slot_after_as_suppresses_identifier_base() {
    fn has_prefix(sql_with_cursor: &str) -> bool {
        sql_with_cursor
            .split_once('|')
            .map(|(b, _)| b.ends_with(|ch: char| ch.is_alphanumeric()))
            .unwrap_or(false)
    }

    // Empty table-alias slots across every table clause: suppress, and fold into
    // the shared keyword-only-identifier-slot family.
    for sql in [
        "SELECT * FROM emp AS |",
        "SELECT * FROM emp e1 JOIN dept AS |",
        "SELECT * FROM (SELECT 1 FROM dual) AS |",
        "UPDATE emp AS |",
        "DELETE FROM emp AS |",
        "MERGE INTO emp AS |",
        "INSERT INTO emp AS |",
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_table_alias_name_slot(&ctx, has_prefix(sql)),
            "table-alias slot should suppress: {sql}"
        );
    }

    // Positions where `AS` is not a table-alias slot: a type slot, the flashback
    // expression after `AS OF`, a select-list alias (handled by its own
    // predicate, not the table one), and a non-`AS` table position.
    for sql in [
        "SELECT * FROM emp AS OF |",        // flashback timestamp expression
        "SELECT col AS | FROM t",           // select-list alias, not a table slot
        "SELECT * FROM emp |",              // no `AS`: ambiguous comma/join/clause slot
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            !SqlEditorWidget::cursor_is_at_table_alias_name_slot(&ctx, has_prefix(sql)),
            "must not be a table-alias slot: {sql}"
        );
    }
}

/// MERGE merge-action slot after `WHEN [NOT] MATCHED [AND <cond>] THEN |`:
/// only `UPDATE`/`DELETE` (matched) or `INSERT` (not matched) are grammatical
/// there — never a column. The `ON (...)` join phase used to bleed into this
/// slot and offer columns. Columns are suppressed and the action keywords are
/// emitted, while a `CASE … THEN` branch inside the statement is unaffected.
#[test]
fn merge_when_then_action_slot_offers_action_keywords_not_columns() {
    let matched =
        "MERGE INTO t USING s ON (t.id = s.id) WHEN MATCHED THEN |";
    let matched_cond =
        "MERGE INTO t USING s ON (t.id = s.id) WHEN MATCHED AND s.x > 0 THEN |";
    let not_matched =
        "MERGE INTO t USING s ON (t.id = s.id) WHEN NOT MATCHED THEN |";
    let case_branch =
        "MERGE INTO t USING s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET y = CASE WHEN s.z = 1 THEN |";

    for (sql, expected) in [
        (matched, vec!["UPDATE".to_string(), "DELETE".to_string()]),
        (matched_cond, vec!["UPDATE".to_string(), "DELETE".to_string()]),
        (not_matched, vec!["INSERT".to_string()]),
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "columns must be suppressed: {sql}"
        );
        assert_eq!(
            SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None),
            expected,
            "{sql}"
        );
    }

    // A `CASE … THEN` branch inside the MERGE action body is an expression slot,
    // not a merge-action slot: it must not be suppressed or offer action verbs.
    let case_ctx = analyze_inline_cursor_sql(case_branch);
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&case_ctx, false));
    assert!(SqlEditorWidget::collect_expected_keyword_suggestions("", &case_ctx, None).is_empty());
}

/// The row-locking clause `… FOR |` (→ `UPDATE`/`SHARE`) and `… FOR UPDATE |`
/// (→ `OF`/`NOWAIT`/…) are keyword-only slots — a column is never valid. The
/// trailing-clause phase used to leave them in a column context. They are gated
/// to the statement top level so the SQL-standard `SUBSTRING(x FROM a FOR |)`
/// operand, the `MODEL`/`PIVOT` `FOR`, and PL/SQL `FOR` loops / `OPEN … FOR`
/// keep their normal completion.
#[test]
fn for_update_locking_clause_is_keyword_only_not_columns() {
    for (sql, expected) in [
        ("SELECT * FROM t FOR |", vec!["UPDATE".to_string(), "SHARE".to_string()]),
        ("SELECT * FROM t WHERE x = 1 FOR |", vec!["UPDATE".to_string(), "SHARE".to_string()]),
        ("SELECT * FROM t ORDER BY x FOR |", vec!["UPDATE".to_string(), "SHARE".to_string()]),
        (
            "SELECT * FROM emp e WHERE e.sal > (SELECT avg(sal) FROM emp) FOR |",
            vec!["UPDATE".to_string(), "SHARE".to_string()],
        ),
        (
            "SELECT * FROM t FOR UPDATE |",
            vec!["OF".to_string(), "NOWAIT".to_string(), "WAIT".to_string(), "SKIP".to_string()],
        ),
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "columns must be suppressed: {sql}"
        );
        assert_eq!(
            SqlEditorWidget::collect_expected_keyword_suggestions("", &ctx, None),
            expected,
            "{sql}"
        );
    }

    // `FOR UPDATE OF |` is a column list — columns must still be offered.
    let of_ctx = analyze_inline_cursor_sql("SELECT * FROM t FOR UPDATE OF |");
    assert!(!SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&of_ctx, false));

    // Not the locking clause: keep normal completion.
    for sql in [
        "SELECT substr(x FROM 1 FOR |) FROM t",        // SUBSTRING operand (in parens)
        "BEGIN FOR | IN (SELECT * FROM t) LOOP NULL; END LOOP; END;", // PL/SQL loop
        "DECLARE BEGIN OPEN c FOR | END;",             // ref-cursor OPEN FOR
    ] {
        let ctx = analyze_inline_cursor_sql(sql);
        assert!(
            !SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, false),
            "must not suppress: {sql}"
        );
    }
}

#[test]
fn data_type_create_table_column_offers_types() {
    let s = data_type_suggestions("CREATE TABLE t (id |)", "", crate::db::DatabaseType::Oracle);
    assert!(s.contains(&"NUMBER".to_string()));
    let s2 = data_type_suggestions("CREATE TABLE t (id NUMBER, name |)", "", crate::db::DatabaseType::Oracle);
    assert!(s2.contains(&"VARCHAR2".to_string()));
}

#[test]
fn data_type_mysql_column_def_uses_full_type_set() {
    let s = data_type_suggestions("CREATE TABLE t (id |)", "", crate::db::DatabaseType::MySQL);
    assert!(s.contains(&"VARCHAR".to_string()));
    assert!(s.contains(&"TEXT".to_string()));
    assert!(s.contains(&"DATETIME".to_string()));
}

#[test]
fn data_type_alter_add_modify_change_column_offers_types() {
    for sql in [
        "ALTER TABLE t ADD col |",
        "ALTER TABLE t MODIFY col |",
        "ALTER TABLE t ADD COLUMN col |",
        "ALTER TABLE t CHANGE old new |",
    ] {
        assert!(
            !data_type_suggestions(sql, "", crate::db::DatabaseType::MySQL).is_empty(),
            "expected type suggestions for: {sql}"
        );
    }
}

#[test]
fn data_type_ordinary_as_alias_is_not_a_type_position() {
    assert!(
        data_type_suggestions("SELECT x AS | FROM t", "", crate::db::DatabaseType::Oracle).is_empty()
    );
}

#[test]
fn data_type_constraint_and_ctas_and_post_type_are_not_type_positions() {
    for sql in [
        "CREATE TABLE t (id NUMBER, CONSTRAINT |)",
        "CREATE TABLE t AS SELECT col | FROM s",
        "CREATE TABLE t (id NUMBER |)",
        "SELECT a, | FROM t",
    ] {
        assert!(
            data_type_suggestions(sql, "", crate::db::DatabaseType::Oracle).is_empty(),
            "expected no type suggestions for: {sql}"
        );
    }
}

/// True when the given marker SQL yields PL/SQL data-type suggestions.
fn has_plsql_type_suggestions(sql_with_cursor: &str, db: crate::db::DatabaseType) -> bool {
    !data_type_suggestions(sql_with_cursor, "", db).is_empty()
}

#[test]
fn data_type_plsql_variable_declaration_offers_types() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "DECLARE v | BEGIN NULL; END;",
        "DECLARE v NUMBER; w | BEGIN NULL; END;",
        "DECLARE v NUMBER; w CONSTANT | BEGIN NULL; END;",
    ] {
        assert!(has_plsql_type_suggestions(sql, Oracle), "expected types for: {sql}");
    }
}

#[test]
fn data_type_plsql_variable_type_list_includes_plsql_only_types() {
    let s = data_type_suggestions("DECLARE v | BEGIN NULL; END;", "", crate::db::DatabaseType::Oracle);
    assert!(s.contains(&"PLS_INTEGER".to_string()));
    assert!(s.contains(&"SYS_REFCURSOR".to_string()));
}

#[test]
fn data_type_plsql_routine_parameter_offers_types() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "CREATE FUNCTION f(p |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE FUNCTION f(p IN |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE FUNCTION f(p IN OUT |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE FUNCTION f(a NUMBER, b |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE PROCEDURE pr(x IN OUT |) IS BEGIN NULL; END;",
    ] {
        assert!(has_plsql_type_suggestions(sql, Oracle), "expected types for: {sql}");
    }
}

#[test]
fn data_type_mysql_routine_parameter_uses_mysql_types() {
    let s = data_type_suggestions("CREATE PROCEDURE pr(x |) BEGIN END", "", crate::db::DatabaseType::MySQL);
    assert!(s.contains(&"VARCHAR".to_string()));
    assert!(s.contains(&"INT".to_string()));
}

#[test]
fn data_type_plsql_function_return_offers_types() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "CREATE FUNCTION f RETURN | IS BEGIN RETURN 1; END;",
        "CREATE FUNCTION f(p NUMBER) RETURN | IS BEGIN RETURN 1; END;",
    ] {
        assert!(has_plsql_type_suggestions(sql, Oracle), "expected return types for: {sql}");
    }
}

#[test]
fn data_type_plsql_collection_element_offers_types() {
    assert!(has_plsql_type_suggestions(
        "DECLARE TYPE t IS TABLE OF | ; BEGIN NULL; END;",
        crate::db::DatabaseType::Oracle
    ));
}

#[test]
fn data_type_plsql_executable_section_is_not_a_type_position() {
    use crate::db::DatabaseType::Oracle;
    // None of these are declaration/signature positions; they must not offer types.
    for sql in [
        "DECLARE v NUMBER; BEGIN v | END;",
        "DECLARE v NUMBER; BEGIN x := v | END;",
        "BEGIN proc | END;",
        "CREATE FUNCTION f RETURN NUMBER IS BEGIN RETURN | END;",
        "DECLARE v NUMBER; BEGIN IF x | THEN NULL; END IF; END;",
        "BEGIN FOR r | IN (SELECT 1 FROM dual) LOOP NULL; END LOOP; END;",
    ] {
        assert!(!has_plsql_type_suggestions(sql, Oracle), "unexpected types for: {sql}");
    }
}

#[test]
fn data_type_in_predicate_is_not_a_parameter_mode() {
    assert!(!has_plsql_type_suggestions(
        "SELECT * FROM t WHERE x IN | ",
        crate::db::DatabaseType::Oracle
    ));
}

#[test]
fn data_type_for_update_of_column_is_not_a_type_position() {
    // `FOR UPDATE OF <col>` is a column list, not a collection element type.
    assert!(!has_plsql_type_suggestions(
        "SELECT col FROM emp FOR UPDATE OF |",
        crate::db::DatabaseType::Oracle
    ));
}

#[test]
fn data_type_create_type_table_of_offers_types() {
    assert!(has_plsql_type_suggestions(
        "CREATE TYPE t AS TABLE OF | ",
        crate::db::DatabaseType::Oracle
    ));
}

#[test]
fn data_type_cursor_body_sql_in_declaration_region_is_not_a_type_position() {
    use crate::db::DatabaseType::Oracle;
    // A `CURSOR c IS SELECT ...` body sits inside a declaration region but is
    // SQL, not a declaration; its clauses must never offer data types.
    for sql in [
        "DECLARE CURSOR c IS SELECT | FROM t; BEGIN NULL; END;",
        "DECLARE CURSOR c IS SELECT a FROM | ; BEGIN NULL; END;",
        "DECLARE v NUMBER := (SELECT max(x) FROM | ); BEGIN NULL; END;",
        "CREATE FUNCTION f RETURN NUMBER IS CURSOR c IS SELECT | FROM t; BEGIN RETURN 1; END;",
    ] {
        assert!(!has_plsql_type_suggestions(sql, Oracle), "unexpected types for: {sql}");
    }
}

#[test]
fn data_type_declaration_after_cursor_or_subtype_still_offers_types() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "DECLARE CURSOR c IS SELECT 1 FROM dual; v | BEGIN NULL; END;",
        "CREATE PROCEDURE p IS firstvar | BEGIN NULL; END;",
        "DECLARE SUBTYPE s IS NUMBER; v | BEGIN NULL; END;",
    ] {
        assert!(has_plsql_type_suggestions(sql, Oracle), "expected types for: {sql}");
    }
}

#[test]
fn data_type_oracle_parenthesized_alter_offers_types() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "ALTER TABLE t ADD (col1 NUMBER, col2 |)",
        "ALTER TABLE t MODIFY (col1 |)",
        "CREATE GLOBAL TEMPORARY TABLE t (id |)",
        "CREATE TABLE t (id NUMBER NOT NULL, name |)",
    ] {
        assert!(
            !data_type_suggestions(sql, "", Oracle).is_empty(),
            "expected types for: {sql}"
        );
    }
}

#[test]
fn data_type_ddl_non_type_slots_offer_nothing() {
    use crate::db::DatabaseType::Oracle;
    for sql in [
        "CREATE TABLE t (id NUMBER DEFAULT |)",
        "ALTER TABLE t DROP COLUMN |",
        "ALTER TABLE t ADD CONSTRAINT pk PRIMARY KEY (|)",
        "CREATE TABLE t (a NUMBER, |)",
        "CREATE TABLE t (id NUMBER, PRIMARY KEY (|))",
        "CREATE INDEX ix ON t (|)",
    ] {
        assert!(
            data_type_suggestions(sql, "", Oracle).is_empty(),
            "unexpected types for: {sql}"
        );
    }
}

#[test]
fn data_type_quoted_column_name_is_a_type_position() {
    assert!(!data_type_suggestions(r#"CREATE TABLE t ("My Col" |)"#, "", crate::db::DatabaseType::Oracle).is_empty());
    assert!(!data_type_suggestions("CREATE TABLE t (`my col` |)", "", crate::db::DatabaseType::MySQL).is_empty());
}

#[test]
fn data_type_value_positions_after_complete_type_are_not_type_slots() {
    use crate::db::DatabaseType::Oracle;
    // A value follows DEFAULT / := , never a type.
    for sql in [
        "CREATE FUNCTION f(p NUMBER DEFAULT |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE FUNCTION f(p IN NUMBER := |) RETURN NUMBER IS BEGIN RETURN 1; END;",
        "CREATE TABLE t (id NUMBER DEFAULT |)",
        "DECLARE v NUMBER := | BEGIN NULL; END;",
        "BEGIN UPDATE t SET a=1 RETURNING a INTO | ; END;",
    ] {
        assert!(!has_plsql_type_suggestions(sql, Oracle), "unexpected types for: {sql}");
    }
}

#[test]
fn data_type_trigger_declaration_offers_types_but_body_does_not() {
    use crate::db::DatabaseType::Oracle;
    assert!(has_plsql_type_suggestions(
        "CREATE TRIGGER trg BEFORE INSERT ON t FOR EACH ROW DECLARE v | BEGIN NULL; END;",
        Oracle
    ));
    assert!(!has_plsql_type_suggestions(
        "CREATE TRIGGER trg BEFORE INSERT ON t FOR EACH ROW BEGIN :NEW.col := | ; END;",
        Oracle
    ));
}

#[test]
fn data_type_declaration_after_rowtype_member_still_offers_types() {
    use crate::db::DatabaseType::Oracle;
    assert!(has_plsql_type_suggestions(
        "DECLARE v emp.sal%TYPE; w | BEGIN NULL; END;",
        Oracle
    ));
}

#[test]
fn data_type_json_table_columns_clause_offers_types() {
    use crate::db::DatabaseType::{Oracle, MySQL};
    assert!(!data_type_suggestions(
        "SELECT * FROM JSON_TABLE(d, '$' COLUMNS (id | PATH '$.id'))", "", Oracle).is_empty());
    assert!(!data_type_suggestions(
        "SELECT * FROM JSON_TABLE(d, '$' COLUMNS (id NUMBER PATH '$.id', name | PATH '$.n'))", "", Oracle).is_empty());
    assert!(!data_type_suggestions(
        "SELECT * FROM XMLTABLE('/r' PASSING x COLUMNS id | PATH 'id')", "", Oracle).is_empty());
    assert!(!data_type_suggestions(
        "SELECT * FROM JSON_TABLE(d, '$' COLUMNS (id | PATH '$.id'))", "", MySQL).is_empty());
}

#[test]
fn data_type_table_named_columns_is_not_a_type_position() {
    use crate::db::DatabaseType::{Oracle, MySQL};
    // A table literally named/aliased around "columns" must never offer types.
    assert!(data_type_suggestions("SELECT * FROM all_tab_columns c |", "", Oracle).is_empty());
    assert!(data_type_suggestions("SELECT * FROM information_schema.columns x |", "", MySQL).is_empty());
    // Before the COLUMNS keyword (the JSON expression) is not a type slot either.
    assert!(data_type_suggestions(
        "SELECT * FROM JSON_TABLE(d| , '$' COLUMNS (id NUMBER PATH '$.id'))", "", Oracle).is_empty());
}

#[test]
fn data_type_prior_statement_does_not_leak_into_next() {
    // A PL/SQL or DDL statement before the cursor's statement must not push its
    // declaration/routine/column context into the next statement.
    for sql in [
        "DECLARE v NUMBER; BEGIN NULL; END;\nSELECT col | FROM t",
        "CREATE FUNCTION f RETURN NUMBER IS BEGIN RETURN 1; END;\nSELECT | FROM t",
        "CREATE TABLE a (x NUMBER);\nSELECT col | FROM t",
        "CREATE FUNCTION f(p IN NUMBER) RETURN NUMBER IS BEGIN RETURN 1; END;\nUPDATE t SET col = | WHERE id=1",
        "BEGIN proc(); END;\nSELECT a, | FROM t",
        "DECLARE CURSOR c IS SELECT 1 FROM dual; BEGIN NULL; END;\nINSERT INTO t (col, |) VALUES (1,2)",
    ] {
        assert!(
            SqlEditorWidget::data_type_position_for_context(&analyze_inline_cursor_sql(sql), false)
                .is_none(),
            "prior statement leaked a type position into: {sql}"
        );
    }
    // The current statement is still detected when it genuinely is a type slot.
    assert!(
        SqlEditorWidget::data_type_position_for_context(
            &analyze_inline_cursor_sql("SELECT col FROM t;\nDECLARE v | BEGIN NULL; END;"),
            false,
        )
        .is_some()
    );
}

#[test]
fn row_count_positions_suppress_columns() {
    let at = |sql: &str| {
        SqlEditorWidget::cursor_is_in_row_limiting_clause_for_context(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };
    // Row-count / offset value slots accept only integers/binds.
    assert!(at("SELECT * FROM orders ORDER BY id LIMIT |"));
    assert!(at("SELECT * FROM orders LIMIT |"));
    assert!(at("SELECT * FROM orders LIMIT 10, |"));
    assert!(at("SELECT * FROM orders LIMIT 10 OFFSET |"));
    assert!(at("SELECT a FROM t OFFSET |"));
    assert!(at("SELECT * FROM orders LIMIT 10 |"));
    assert!(at("SELECT * FROM orders LIMIT 10 OFFSET 20 |"));
    assert!(at("SELECT * FROM orders LIMIT 10, 20 |"));
    // The whole Oracle/ANSI FETCH/OFFSET row-limiting tail is a no-column zone:
    // count slots, unit slots, PERCENT, and the ONLY/WITH/TIES keyword slots all
    // collapse to OrderByClause yet never accept a column.
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH NEXT |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH NEXT :n |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 PERCENT |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS WITH |"));
    assert!(at("SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS ONLY |"));
    assert!(at(
        "SELECT * FROM emp ORDER BY empno FETCH FIRST 5 ROWS WITH TIES |"
    ));
    assert!(at(
        "SELECT * FROM emp OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY |"
    ));
    assert!(at("SELECT * FROM emp OFFSET 10 |"));
    assert!(at("SELECT * FROM emp OFFSET 10 ROWS |"));

    let kw = |sql: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::MySQL),
        )
    };
    assert_eq!(
        kw("SELECT * FROM orders LIMIT 10 |"),
        vec!["OFFSET".to_string()]
    );
    assert!(kw("SELECT * FROM orders LIMIT 10 OFFSET 20 |").is_empty());
    assert!(kw("SELECT limit 10 | FROM orders").is_empty());

    // Ordinary column positions are unaffected, including a `offset_*` column.
    assert!(!at("SELECT | FROM orders"));
    assert!(!at("SELECT a, | FROM orders"));
    assert!(!at("SELECT * FROM orders WHERE | "));
    assert!(!at("SELECT offset_days, | FROM t"));
    assert!(!at("SELECT limit | FROM orders"));
    assert!(!at("SELECT offset | FROM orders"));
    assert!(!at("SELECT * FROM orders ORDER BY |"));
}

#[test]
fn local_record_member_scope_boundary_and_nested_loops() {
    let members = |sql: &str, q: &str| {
        SqlEditorWidget::collect_local_record_member_suggestions_for_test(sql, q, "")
    };
    // A loop record is not visible outside its own loop.
    assert!(members(
        "BEGIN FOR rec IN (SELECT a FROM t) LOOP NULL; END LOOP; rec.__CODEX_CURSOR__ END;",
        "rec",
    )
    .is_none());
    // Inner loop sees its own record and the enclosing loop's record.
    let inner = members(
        "BEGIN FOR a IN (SELECT x FROM t1) LOOP FOR b IN (SELECT y FROM t2) LOOP b.__CODEX_CURSOR__ END LOOP; END LOOP; END;",
        "b",
    )
    .expect("inner loop record visible");
    assert_has_case_insensitive(&inner, "y");
    let outer = members(
        "BEGIN FOR a IN (SELECT x FROM t1) LOOP FOR b IN (SELECT y FROM t2) LOOP a.__CODEX_CURSOR__ END LOOP; END LOOP; END;",
        "a",
    )
    .expect("enclosing loop record visible inside inner loop");
    assert_has_case_insensitive(&outer, "x");
}


/// The base catalog's flat, prefix-only keyword dump is filtered down to an
/// allowlist of keywords that are actually grammatical at the cursor's
/// expression position: clause/statement/DDL keywords never appear in a value
/// expression, construct-scoped keywords (CASE body, window-frame bounds, MERGE
/// `MATCHED`) appear only inside their construct, operators appear only after a
/// complete operand, and value/function keywords only where an operand is
/// expected. Columns, relations and objects stay scoped to operand positions
/// too, and a real column named like a keyword is never hidden.
#[test]
fn expression_keyword_completion_is_position_aware() {
    use crate::db::DatabaseType::Oracle;

    fn suggestions(sql_with_cursor: &str, extra_emp_columns: &[&str]) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let context =
            SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        let mut emp_columns = vec!["ENAME".to_string(), "EMPNO".to_string()];
        emp_columns.extend(extra_emp_columns.iter().map(|c| c.to_string()));
        data.set_columns_for_table("EMP", emp_columns);
        data.rebuild_indices();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data,
            &prefix,
            None,
            column_scope.as_deref(),
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            context,
            false,
            Some(Oracle),
            expr_keyword_ctx,
        )
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // Outside a CASE the body keywords are pure noise and must be dropped, while
    // the real columns of the position still come through.
    let s = suggestions("SELECT en| FROM emp", &[]);
    assert!(!has(&s, "END"), "END leaked outside CASE: {s:?}");
    assert!(has(&s, "ENAME"), "real column dropped: {s:?}");
    for (sql, kw) in [
        ("SELECT el| FROM emp", "ELSE"),
        ("SELECT el| FROM emp", "ELSIF"),
        ("SELECT * FROM emp WHERE th|", "THEN"),
        ("SELECT * FROM emp WHERE en| = 1", "END"),
        ("SELECT ename FROM emp GROUP BY th|", "THEN"),
    ] {
        let s = suggestions(sql, &[]);
        assert!(!has(&s, kw), "{kw} leaked outside CASE for `{sql}`: {s:?}");
    }

    // Inside an unclosed CASE the body keywords are grammatical again.
    let s = suggestions("SELECT CASE WHEN empno = 1 th| FROM emp", &[]);
    assert!(has(&s, "THEN"), "THEN suppressed inside CASE: {s:?}");
    let s = suggestions("SELECT CASE WHEN empno = 1 THEN 2 en| FROM emp", &[]);
    assert!(has(&s, "END"), "END suppressed inside CASE: {s:?}");
    let s = suggestions("SELECT CASE WHEN empno = 1 THEN 2 el| FROM emp", &[]);
    assert!(has(&s, "ELSE"), "ELSE suppressed inside CASE: {s:?}");

    // A column literally named like a CASE keyword is preserved even outside a
    // CASE — the filter never hides a legitimate completion.
    let s = suggestions("SELECT en| FROM emp", &["END"]);
    assert!(has(&s, "END"), "column named END was hidden: {s:?}");

    // The same flat-catalog problem applies to the window-frame boundary
    // keywords (`PRECEDING`/`FOLLOWING`/`UNBOUNDED`): they are only grammatical
    // inside a window specification's frame clause, so outside any window spec
    // they are stripped, but inside an `OVER (...)` / `WINDOW … AS (...)` they
    // remain available.
    for (sql, kw) in [
        ("SELECT pr| FROM emp", "PRECEDING"),
        ("SELECT fo| FROM emp", "FOLLOWING"),
        ("SELECT un| FROM emp", "UNBOUNDED"),
        ("SELECT * FROM emp WHERE pr| = 1", "PRECEDING"),
    ] {
        let s = suggestions(sql, &[]);
        assert!(!has(&s, kw), "{kw} leaked outside window spec for `{sql}`: {s:?}");
    }
    let s = suggestions(
        "SELECT SUM(empno) OVER (ORDER BY empno ROWS BETWEEN un| FROM emp",
        &[],
    );
    assert!(has(&s, "UNBOUNDED"), "UNBOUNDED suppressed inside window frame: {s:?}");
    let s = suggestions(
        "SELECT SUM(empno) OVER (ORDER BY empno ROWS BETWEEN 1 pr| FROM emp",
        &[],
    );
    assert!(has(&s, "PRECEDING"), "PRECEDING suppressed inside window frame: {s:?}");
    // A column named like a frame keyword is preserved outside a window spec.
    let s = suggestions("SELECT pr| FROM emp", &["PRECEDING"]);
    assert!(has(&s, "PRECEDING"), "column named PRECEDING was hidden: {s:?}");

    // The MERGE action keyword `MATCHED` only follows `WHEN [NOT]`; it must not
    // leak into a value/column position.
    for sql in ["SELECT ma| FROM emp", "SELECT * FROM emp WHERE ma| = 1"] {
        let s = suggestions(sql, &[]);
        assert!(!has(&s, "MATCHED"), "MATCHED leaked into value position for `{sql}`: {s:?}");
    }
    // Its legitimate MERGE slot is still served by the contextual keyword merge.
    let ctx = analyze_inline_cursor_sql(
        "MERGE INTO emp e USING dept d ON (e.empno = d.deptno) WHEN ma|",
    );
    let kw = SqlEditorWidget::collect_expected_keyword_suggestions("ma", &ctx, Some(Oracle));
    assert!(
        kw.iter().any(|k| k.eq_ignore_ascii_case("MATCHED")),
        "MERGE WHEN slot lost MATCHED: {kw:?}"
    );
    // A column named MATCHED is preserved in a value position.
    let s = suggestions("SELECT ma| FROM emp", &["MATCHED"]);
    assert!(has(&s, "MATCHED"), "column named MATCHED was hidden: {s:?}");

    // Clause / statement / DDL keywords never belong in a value expression and
    // are dropped from the base dump (their real slots are served by the
    // grammar-aware keyword merge).
    for (sql, kw) in [
        ("SELECT cr| FROM emp", "CREATE"),
        ("SELECT fr| FROM emp", "FROM"),
        ("SELECT ta| FROM emp", "TABLESPACE"),
        ("SELECT wh| FROM emp", "WHERE"),
        ("SELECT * FROM emp WHERE ad| = 1", "ADD"),
        ("SELECT gr| FROM emp", "GROUP"),
    ] {
        let s = suggestions(sql, &[]);
        assert!(!has(&s, kw), "{kw} leaked into value expression for `{sql}`: {s:?}");
    }

    // Genuine expression keywords/functions survive where an operand is expected.
    let s = suggestions("SELECT ca| FROM emp", &[]);
    assert!(has(&s, "CASE") && has(&s, "CAST"), "expression starters dropped: {s:?}");
    let s = suggestions("SELECT ex| FROM emp", &[]);
    assert!(has(&s, "EXISTS"), "EXISTS dropped at operand start: {s:?}");
    let s = suggestions("SELECT co| FROM emp", &[]);
    assert!(has(&s, "COALESCE"), "function keyword dropped at operand start: {s:?}");

    // After a complete operand only operators are grammatical: no functions, no
    // columns, no operand-starters.
    let s = suggestions("SELECT ename a| FROM emp", &[]);
    assert!(has(&s, "AND"), "operator dropped after operand: {s:?}");
    let s = suggestions("SELECT ename ab| FROM emp", &[]);
    assert!(!has(&s, "ABS()") && !has(&s, "ABS"), "function leaked after operand: {s:?}");
    // The implicit-alias slot (`<expr> <name>|`) no longer leaks columns/tables.
    let s = suggestions("SELECT ename e| FROM emp", &[]);
    for leaked in ["ENAME", "EMPNO", "EMP"] {
        assert!(!has(&s, leaked), "{leaked} leaked into implicit-alias slot: {s:?}");
    }

    // Operators are not offered where an operand is expected (statement start).
    let s = suggestions("SELECT a| FROM emp", &[]);
    assert!(!has(&s, "AND") && !has(&s, "AT"), "operator offered at operand start: {s:?}");
    // Columns are still offered where an operand is expected.
    let s = suggestions("SELECT * FROM emp WHERE en| = 1", &[]);
    assert!(has(&s, "ENAME"), "column dropped at operand-start: {s:?}");
}

/// The analytic/aggregate continuations `OVER`, `KEEP` and `WITHIN` (GROUP) are
/// only grammatical immediately after a closed call — `SUM(x) OVER (…)`,
/// `MAX(x) KEEP (DENSE_RANK …)`, `LISTAGG(…) WITHIN GROUP (…)`. After any other
/// complete operand (a plain column, a literal, a parenthesized value) they are
/// pure noise. They always follow a `)`, so gating them on a closed call removes
/// the noise without ever hiding a valid completion.
#[test]
fn analytic_continuations_offered_only_after_a_closed_call() {
    fn suggestions(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let context =
            SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.set_columns_for_table("EMP", vec!["ENAME".to_string(), "EMPNO".to_string()]);
        data.rebuild_indices();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data,
            &prefix,
            None,
            column_scope.as_deref(),
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            context,
            false,
            Some(crate::db::DatabaseType::Oracle),
            expr_keyword_ctx,
        )
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // After a plain column / literal operand the continuations are noise.
    for (sql, kw) in [
        ("SELECT ename ov| FROM emp", "OVER"),
        ("SELECT ename ke| FROM emp", "KEEP"),
        ("SELECT ename wi| FROM emp", "WITHIN"),
        ("SELECT * FROM emp WHERE empno ov| = 1", "OVER"),
        ("SELECT 1 ov| FROM emp", "OVER"),
    ] {
        let s = suggestions(sql);
        assert!(!has(&s, kw), "{kw} leaked after a plain operand for `{sql}`: {s:?}");
    }

    // A closing `)` that ends a *grouping/predicate paren* or a *subquery* — not
    // a function call — is not a call site: the continuations stay suppressed.
    // (Regression: the previous "last token is `)`" test treated every paren as a
    // call, leaking `OVER`/`KEEP`/`WITHIN` after `(a + b)`, `(SELECT …)`, etc.)
    for (sql, kw) in [
        ("SELECT (empno + 1) ov| FROM emp", "OVER"),
        ("SELECT (sum(empno)) ov| FROM emp", "OVER"),
        ("SELECT (SELECT max(empno) FROM emp) ov| FROM emp", "OVER"),
        ("SELECT (empno) ke| FROM emp", "KEEP"),
        ("SELECT (empno) wi| FROM emp", "WITHIN"),
        ("SELECT * FROM emp WHERE EXISTS (SELECT 1 FROM emp) ov|", "OVER"),
        ("SELECT * FROM emp WHERE empno IN (1, 2) ov|", "OVER"),
    ] {
        let s = suggestions(sql);
        assert!(!has(&s, kw), "{kw} leaked after a non-call `)` for `{sql}`: {s:?}");
    }

    // A user-defined routine call (a non-keyword identifier before `(`) is still
    // a call site, so the continuation survives.
    assert!(
        has(&suggestions("SELECT my_rank(empno) ov| FROM emp"), "OVER"),
        "OVER suppressed after a user-defined function call"
    );

    // Immediately after a closed call they are grammatical again.
    for (sql, kw) in [
        ("SELECT sum(empno) ov| FROM emp", "OVER"),
        ("SELECT max(empno) ke| FROM emp", "KEEP"),
        ("SELECT count(*) ov| FROM emp", "OVER"),
        ("SELECT listagg(ename) wi| FROM emp", "WITHIN"),
    ] {
        let s = suggestions(sql);
        assert!(has(&s, kw), "{kw} suppressed after a closed call for `{sql}`: {s:?}");
    }

    // The MySQL full-text continuation `AGAINST` belongs to the same family:
    // `MATCH(col) AGAINST ('text')` — a closed call, never a plain operand.
    let mysql = |sql_with_cursor: &str| -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let context =
            SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.set_columns_for_table("EMP", vec!["ENAME".to_string(), "EMPNO".to_string()]);
        data.rebuild_indices();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::MySQL));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data,
            &prefix,
            None,
            column_scope.as_deref(),
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            context,
            false,
            Some(crate::db::DatabaseType::MySQL),
            expr_keyword_ctx,
        )
    };
    assert!(
        !has(&mysql("SELECT ename ag| FROM emp"), "AGAINST"),
        "AGAINST leaked after a plain operand"
    );
    assert!(
        has(&mysql("SELECT match(ename) ag| FROM emp"), "AGAINST"),
        "AGAINST suppressed after a closed call"
    );
}

/// `ESCAPE` is grammatical only right after a `LIKE` pattern
/// (`name LIKE 'a\_%' ESCAPE '\'`), never after a plain operand. It is gated on
/// the same "continuation that needs a specific preceding operand" principle as
/// `OVER`/`KEEP`/`WITHIN`, so the same noise must not leak.
#[test]
fn escape_offered_only_after_a_like_pattern() {
    fn suggestions(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let context =
            SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.set_columns_for_table("EMP", vec!["ENAME".to_string(), "EMPNO".to_string()]);
        data.rebuild_indices();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data,
            &prefix,
            None,
            column_scope.as_deref(),
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            context,
            false,
            Some(crate::db::DatabaseType::Oracle),
            expr_keyword_ctx,
        )
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // No preceding LIKE in the current predicate segment → ESCAPE is noise.
    for sql in [
        "SELECT * FROM emp WHERE ename es| ",
        "SELECT * FROM emp WHERE ename = 'a' es| ",
        // A LIKE in a *different* predicate (across an AND) does not carry over.
        "SELECT * FROM emp WHERE ename LIKE 'a%' AND empno es| ",
        // A LIKE buried in a deeper paren level does not count.
        "SELECT * FROM emp WHERE upper(ename) es| ",
    ] {
        let s = suggestions(sql);
        assert!(!has(&s, "ESCAPE"), "ESCAPE leaked without a LIKE pattern for `{sql}`: {s:?}");
    }

    // Right after a LIKE pattern → ESCAPE is grammatical.
    for sql in [
        "SELECT * FROM emp WHERE ename LIKE 'a%' es| ",
        "SELECT * FROM emp WHERE ename NOT LIKE 'a%' es| ",
        "SELECT * FROM emp WHERE empno = 1 AND ename LIKE 'a%' es| ",
    ] {
        let s = suggestions(sql);
        assert!(has(&s, "ESCAPE"), "ESCAPE suppressed after a LIKE pattern for `{sql}`: {s:?}");
    }
}

/// The set-quantifiers `DISTINCT`/`UNIQUE`/`DISTINCTROW` are grammatical only at
/// the start of a select list or an aggregate argument — right after `SELECT`, a
/// set operator, or an opening `(` — never as a general expression operand. The
/// quantified-comparison keywords `ALL`/`ANY`/`SOME` are deliberately *not*
/// gated, since `x = ANY (...)` is valid.
#[test]
fn set_quantifiers_offered_only_at_a_list_or_aggregate_anchor() {
    fn suggestions(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let context =
            SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.set_columns_for_table("EMP", vec!["ENAME".to_string(), "EMPNO".to_string()]);
        data.rebuild_indices();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data, &prefix, None, column_scope.as_deref(),
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            context, false, Some(crate::db::DatabaseType::Oracle), expr_keyword_ctx,
        )
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // General operand positions: DISTINCT/UNIQUE are noise. The opening `(`
    // cases are *grouping/predicate* parens, not aggregate-call parens, so the
    // quantifier must not be offered just because the previous token is `(`.
    for sql in [
        "SELECT * FROM emp WHERE empno = dis|",
        "SELECT empno + dis| FROM emp",
        "SELECT nvl(empno, dis| FROM emp",
        "SELECT ename, dis| FROM emp",
        "SELECT * FROM emp WHERE empno = uni|",
        "SELECT (dis| FROM emp",
        "SELECT (empno + 1) * (dis| FROM emp",
        "SELECT * FROM emp WHERE (dis|",
    ] {
        let s = suggestions(sql);
        assert!(!has(&s, "DISTINCT"), "DISTINCT leaked into a general operand for `{sql}`: {s:?}");
        assert!(!has(&s, "UNIQUE"), "UNIQUE leaked into a general operand for `{sql}`: {s:?}");
    }

    // List / aggregate anchors: DISTINCT is grammatical.
    for sql in [
        "SELECT dis| FROM emp",
        "SELECT count(dis| FROM emp",
        "SELECT empno FROM emp UNION dis|",
    ] {
        let s = suggestions(sql);
        assert!(has(&s, "DISTINCT"), "DISTINCT suppressed at a valid anchor for `{sql}`: {s:?}");
    }

    // `ANY`/`SOME` remain available as quantified comparisons after `=`.
    let s = suggestions("SELECT * FROM emp WHERE empno = an|");
    assert!(has(&s, "ANY"), "ANY wrongly suppressed in a quantified comparison: {s:?}");
    let s = suggestions("SELECT * FROM emp WHERE empno = al|");
    assert!(has(&s, "ALL"), "ALL wrongly suppressed in a quantified comparison: {s:?}");
}

/// `EMP` with a typed schema (`ENAME` character, `EMPNO` numeric, `HIREDATE`
/// datetime) so context-dependent keyword gating can be exercised end to end.
fn typed_emp_suggestions(sql_with_cursor: &str) -> Vec<String> {
    let cursor = sql_with_cursor.find('|').expect("cursor marker");
    let sql = sql_with_cursor.replace('|', "");
    let ctx = analyze_inline_cursor_sql(sql_with_cursor);
    let context =
        SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
    let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
    let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
    let column_scope = (!column_tables.is_empty()).then(|| column_tables.clone());
    let mut data = IntellisenseData::new();
    data.tables = vec!["EMP".to_string()];
    data.set_columns_for_table(
        "EMP",
        vec!["ENAME".to_string(), "EMPNO".to_string(), "HIREDATE".to_string()],
    );
    let meta = HashMap::from([
        ("ENAME".to_string(), ColumnMeta { type_display: "VARCHAR2(10)".to_string(), nullable: true, is_primary_key: false }),
        ("EMPNO".to_string(), ColumnMeta { type_display: "NUMBER(4)".to_string(), nullable: false, is_primary_key: true }),
        ("HIREDATE".to_string(), ColumnMeta { type_display: "DATE".to_string(), nullable: true, is_primary_key: false }),
    ]);
    data.set_column_meta_for_table("EMP", meta);
    data.rebuild_indices();
    let expr_keyword_ctx =
        SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
    SqlEditorWidget::base_suggestions_for_context(
        &mut data, &prefix, None, column_scope.as_deref(),
        matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
        context, false, Some(crate::db::DatabaseType::Oracle), expr_keyword_ctx,
    )
}

/// Hierarchical pseudo-columns/operators (`LEVEL`, `PRIOR`, `CONNECT_BY_ROOT`,
/// `CONNECT_BY_ISCYCLE`, `CONNECT_BY_ISLEAF`) are grammatical only in a query that
/// has a `CONNECT BY` clause. `ROWNUM` stays valid everywhere.
#[test]
fn hierarchical_keywords_offered_only_in_a_connect_by_query() {
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // No CONNECT BY → hierarchical keywords are noise.
    for (sql, kw) in [
        ("SELECT lev| FROM emp", "LEVEL"),
        ("SELECT * FROM emp WHERE empno = lev|", "LEVEL"),
        ("SELECT conn| FROM emp", "CONNECT_BY_ROOT"),
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(!has(&s, kw), "{kw} leaked outside a CONNECT BY query for `{sql}`: {s:?}");
    }

    // With CONNECT BY → grammatical.
    for (sql, kw) in [
        ("SELECT lev| FROM emp CONNECT BY PRIOR empno = empno", "LEVEL"),
        ("SELECT empno FROM emp CONNECT BY pri|", "PRIOR"),
        ("SELECT conn| FROM emp CONNECT BY PRIOR empno = empno", "CONNECT_BY_ROOT"),
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(has(&s, kw), "{kw} suppressed inside a CONNECT BY query for `{sql}`: {s:?}");
    }

    // ROWNUM stays valid without CONNECT BY.
    let s = typed_emp_suggestions("SELECT * FROM emp WHERE rown| = 1");
    assert!(has(&s, "ROWNUM"), "ROWNUM wrongly suppressed: {s:?}");

    // A `CONNECT BY` clause belongs to its own query level: one nested inside a
    // subquery (`… IN (SELECT … CONNECT BY …)`) must not make the hierarchical
    // keywords grammatical in the *outer* query, whose select list / predicates
    // have no `CONNECT BY` of their own.
    for (sql, kw) in [
        (
            "SELECT lev| FROM emp WHERE empno IN (SELECT empno FROM emp CONNECT BY PRIOR empno = empno)",
            "LEVEL",
        ),
        (
            "SELECT conn| FROM emp WHERE empno IN (SELECT empno FROM emp CONNECT BY PRIOR empno = empno)",
            "CONNECT_BY_ROOT",
        ),
        (
            "SELECT empno FROM emp WHERE empno IN (SELECT empno FROM emp CONNECT BY PRIOR empno = empno) AND empno = pri|",
            "PRIOR",
        ),
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(
            !has(&s, kw),
            "{kw} leaked from a nested CONNECT BY subquery into the outer query for `{sql}`: {s:?}"
        );
    }
}

/// The `DEFAULT` value keyword is grammatical only in a DML value position
/// (`INSERT … VALUES (…)`, `UPDATE … SET col = …`), never in a query expression.
#[test]
fn default_value_keyword_offered_only_in_dml_value_positions() {
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    for sql in [
        "UPDATE emp SET ename = def|",
        "INSERT INTO emp VALUES (def|",
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(has(&s, "DEFAULT"), "DEFAULT suppressed in a DML value position for `{sql}`: {s:?}");
    }
    for sql in [
        "SELECT def| FROM emp",
        "SELECT * FROM emp WHERE empno = def|",
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(!has(&s, "DEFAULT"), "DEFAULT leaked into a query expression for `{sql}`: {s:?}");
    }
}

/// The operand-type postfix operators are gated on the inferred type of the
/// preceding operand: `AT` (`… AT TIME ZONE`) needs a datetime, `COLLATE` a
/// character value. After an operand of the wrong type — or one whose type is
/// unknown — they are withheld.
#[test]
fn operand_type_operators_match_the_preceding_operand_type() {
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // AT after a datetime operand (column, literal, or niladic function).
    for sql in [
        "SELECT * FROM emp WHERE hiredate a|",
        "SELECT sysdate a| FROM emp",
        "SELECT DATE '2020-01-01' a| FROM emp",
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(has(&s, "AT"), "AT suppressed after a datetime operand for `{sql}`: {s:?}");
    }
    // AT after a non-datetime operand is noise.
    for sql in [
        "SELECT * FROM emp WHERE empno a|",
        "SELECT ename a| FROM emp",
        "SELECT 'x' a| FROM emp",
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(!has(&s, "AT"), "AT leaked after a non-datetime operand for `{sql}`: {s:?}");
    }

    // COLLATE after a character operand only.
    let s = typed_emp_suggestions("SELECT ename col| FROM emp");
    assert!(has(&s, "COLLATE"), "COLLATE suppressed after a character operand: {s:?}");
    let s = typed_emp_suggestions("SELECT empno col| FROM emp");
    assert!(!has(&s, "COLLATE"), "COLLATE leaked after a numeric operand: {s:?}");
    let s = typed_emp_suggestions("SELECT hiredate col| FROM emp");
    assert!(!has(&s, "COLLATE"), "COLLATE leaked after a datetime operand: {s:?}");

    // When the operand's type cannot be resolved (an unknown identifier, not an
    // in-scope typed column) the operators are kept — a provable mismatch is
    // required to suppress, so a valid completion is never hidden.
    let s = typed_emp_suggestions("SELECT unknown_col a| FROM emp");
    assert!(has(&s, "AT"), "AT wrongly suppressed after an unknown operand: {s:?}");
    let s = typed_emp_suggestions("SELECT unknown_col col| FROM emp");
    assert!(has(&s, "COLLATE"), "COLLATE wrongly suppressed after an unknown operand: {s:?}");

    // But never at an operand-start (these are postfix operators).
    let s = typed_emp_suggestions("SELECT * FROM emp WHERE empno = a|");
    assert!(!has(&s, "AT"), "AT leaked at an operand-start: {s:?}");
}

/// `FIRST`/`LAST` are not standalone-callable functions in Oracle — they only
/// occur as syntax keywords (`… KEEP (DENSE_RANK FIRST …)`, `NULLS FIRST/LAST`).
/// They must therefore not be treated as value-producing functions and leak into
/// an operand position, but must stay available where they are grammatical.
#[test]
fn first_last_are_not_value_functions() {
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // Not offered as an operand (the old `is_language_function` leak).
    let s = typed_emp_suggestions("SELECT * FROM emp WHERE empno = fir|");
    assert!(!has(&s, "FIRST"), "FIRST leaked into an operand position: {s:?}");
    let s = typed_emp_suggestions("SELECT las| FROM emp");
    assert!(!has(&s, "LAST"), "LAST leaked into an operand position: {s:?}");

    // Still served where grammatical: the `NULLS FIRST/LAST` ordering slot.
    let ctx = analyze_inline_cursor_sql("SELECT * FROM emp ORDER BY ename NULLS fir|");
    let kw = SqlEditorWidget::collect_expected_keyword_suggestions(
        "fir",
        &ctx,
        Some(crate::db::DatabaseType::Oracle),
    );
    assert!(
        kw.iter().any(|k| k.eq_ignore_ascii_case("FIRST")),
        "NULLS FIRST ordering slot lost FIRST: {kw:?}"
    );
}

#[test]
fn keep_dense_rank_keyword_slots_suppress_columns_and_offer_fixed_keywords() {
    let kw = |sql: &str, prefix: &str| {
        SqlEditorWidget::collect_expected_keyword_suggestions(
            prefix,
            &analyze_inline_cursor_sql(sql),
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    let suppresses = |sql: &str| {
        SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(
            &analyze_inline_cursor_sql(sql),
            false,
        )
    };

    assert_eq!(
        kw("SELECT max(empno) KEEP (DENSE_RANK |) FROM emp", ""),
        vec!["FIRST".to_string(), "LAST".to_string()]
    );
    assert_eq!(
        kw("SELECT max(empno) KEEP (DENSE_RANK fir|) FROM emp", "fir"),
        vec!["FIRST".to_string()]
    );
    assert_eq!(
        kw("SELECT max(empno) KEEP (DENSE_RANK FIRST |) FROM emp", ""),
        vec!["ORDER".to_string()]
    );
    assert_eq!(
        kw("SELECT max(empno) KEEP (DENSE_RANK LAST |) FROM emp", ""),
        vec!["ORDER".to_string()]
    );
    assert!(suppresses("SELECT max(empno) KEEP (DENSE_RANK |) FROM emp"));
    assert!(suppresses("SELECT max(empno) KEEP (DENSE_RANK FIRST |) FROM emp"));

    // Outside KEEP's dense-rank aggregate syntax these words keep their normal
    // meaning and do not become keyword-only slots.
    assert!(!suppresses("SELECT dense_rank | FROM emp"));
    assert!(kw("SELECT dense_rank | FROM emp", "").is_empty());
}

/// The top-level statement keywords (`SELECT`, `INSERT`, `CREATE`, `BEGIN`, …)
/// are grammatical only where a new statement can begin. They must never appear
/// mid-clause. The regression: `previous_meaningful_words_upper` stops at a
/// value token, so an operand whose last token is a string literal left the
/// keyword machinery with an empty word list, which it mistook for a statement
/// start and dumped the statement keywords into a `WHERE`/`VALUES`/`IN` slot.
#[test]
fn statement_keywords_offered_only_at_a_real_statement_start() {
    let kw = |sql: &str| {
        let ctx = analyze_inline_cursor_sql(sql);
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &ctx,
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // A genuine statement start still offers them.
    let s = kw("|");
    assert!(has(&s, "SELECT") && has(&s, "CREATE") && has(&s, "BEGIN"),
        "statement start lost the top-level keywords: {s:?}");

    // Mid-expression, right after a value operand — pure noise. A preceding
    // string literal, a parenthesised value list and a `VALUES` row all used to
    // leak the statement keywords through the empty-word-list path.
    for sql in [
        "SELECT * FROM emp WHERE ename = 'x' |",
        "SELECT ename FROM emp WHERE ename LIKE 'a%' |",
        "SELECT * FROM emp WHERE empno IN ('a', |",
        "INSERT INTO emp VALUES ('a', |",
    ] {
        let s = kw(sql);
        for keyword in ["SELECT", "INSERT", "UPDATE", "CREATE", "ALTER", "DROP", "BEGIN", "MERGE"] {
            assert!(
                !has(&s, keyword),
                "{keyword} leaked mid-clause for `{sql}`: {s:?}"
            );
        }
    }
}

/// A column wildcard (`*` / `t.*`) is column/operand material. It must be
/// suppressed in a column-suppressing keyword-only slot (an `EXTRACT` field, a
/// data-type slot, …) and right after a complete operand, while staying
/// available at an operand-start select position.
#[test]
fn select_wildcard_is_position_aware() {
    // Mirrors the apply-path wildcard gate.
    fn wildcard(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let has_prefix = !prefix.is_empty();
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let data = IntellisenseData::new();
        let expr_keyword_ctx =
            SqlEditorWidget::expression_keyword_context(&ctx, &data, &column_tables, has_prefix, None);
        let at_keyword_only_slot =
            SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, has_prefix);
        if at_keyword_only_slot || expr_keyword_ctx.follows_operand == Some(true) {
            Vec::new()
        } else {
            SqlEditorWidget::collect_clause_wildcard_suggestions(&prefix, None, &ctx)
        }
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // Operand-start select positions keep the wildcard.
    assert!(has(&wildcard("SELECT | FROM emp"), "*"), "wildcard lost at select start");
    assert!(has(&wildcard("SELECT ename, | FROM emp"), "*"), "wildcard lost after a comma");

    // Right after a complete select item, the wildcard is noise.
    assert!(wildcard("SELECT ename | FROM emp").is_empty(), "wildcard leaked after a column operand");
    assert!(wildcard("SELECT 'x' | FROM emp").is_empty(), "wildcard leaked after a literal operand");

    // A keyword-only value slot (EXTRACT field) never admits the wildcard.
    assert!(wildcard("SELECT EXTRACT(| FROM hiredate) FROM emp").is_empty(),
        "wildcard leaked into the EXTRACT field slot");
}

/// Operand material (columns, `*`, bare identifiers) is grammatical only where a
/// new operand is expected. With an empty prefix the cursor sits *after* the
/// completed operand, so it must be dropped — the regression was that the
/// expression-keyword context excluded the finished operand from its window and
/// misread the position as an operand-start, leaking columns after
/// `SELECT ename `, `ORDER BY ename `, `WHERE c = 'x' `.
#[test]
fn columns_suppressed_immediately_after_a_complete_operand() {
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // After a complete operand (empty prefix): no operand material.
    for sql in [
        "SELECT ename | FROM emp",
        "SELECT * FROM emp ORDER BY ename |",
        "SELECT * FROM emp WHERE ename = 'x' |",
        "SELECT * FROM emp WHERE empno = empno |",
    ] {
        let s = typed_emp_suggestions(sql);
        assert!(
            !has(&s, "ENAME") && !has(&s, "EMPNO") && !has(&s, "HIREDATE"),
            "operand material leaked after a complete operand for `{sql}`: {s:?}"
        );
    }

    // At an operand-start the same columns are still offered.
    assert!(has(&typed_emp_suggestions("SELECT | FROM emp"), "ENAME"),
        "columns lost at an operand-start select position");
    assert!(has(&typed_emp_suggestions("SELECT * FROM emp WHERE empno = | "), "ENAME"),
        "columns lost after a comparison operator");
    // And typing a column prefix still completes it.
    assert!(has(&typed_emp_suggestions("SELECT en| FROM emp"), "ENAME"),
        "column prefix completion broke");
}

/// The unqualified select-list wildcard (`*`, `t.*`) belongs at a query's own
/// select-list level, never inside a function-call / expression sub-paren. The
/// regression: `SELECT TO_CHAR(x, |)` and `OVER (PARTITION BY |)` leaked `*` and
/// `emp.*`. The aggregate `COUNT(*)` form (`*` right after `(`) and a nested
/// subquery's own select list (`… IN (SELECT | …)`) must keep the wildcard.
#[test]
fn select_wildcard_respects_paren_nesting() {
    fn wildcard(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        SqlEditorWidget::collect_clause_wildcard_suggestions(&prefix, None, &ctx)
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x == s);

    // A query's own select-list level keeps the full wildcard set.
    assert!(has(&wildcard("SELECT | FROM emp"), "*"));
    assert!(has(&wildcard("SELECT | FROM emp"), "emp.*"));
    assert!(has(&wildcard("SELECT ename, | FROM emp"), "*"));
    // A nested subquery's own select list keeps it too (correlation-aware).
    assert!(has(&wildcard("SELECT * FROM emp WHERE deptno IN (SELECT | FROM dept)"), "*"));
    assert!(has(&wildcard("SELECT (SELECT | FROM dept) FROM emp"), "*"));

    // A function-call / expression sub-paren admits no projection wildcard.
    for sql in [
        "SELECT TO_CHAR(hiredate, |) FROM emp",
        "SELECT SUBSTR(ename, |) FROM emp",
        "SELECT COUNT(DISTINCT |) FROM emp",
        "SELECT SUM(sal) OVER (PARTITION BY |) FROM emp",
    ] {
        assert!(wildcard(sql).is_empty(), "wildcard leaked into a sub-paren: {sql}");
    }

    // The aggregate `COUNT(*)` form survives — bare `*` only, no `t.*`.
    let count = wildcard("SELECT COUNT(|) FROM emp");
    assert!(has(&count, "*"), "COUNT(*) lost its star");
    assert!(!has(&count, "emp.*"), "COUNT( must not offer a qualified wildcard");
}

/// A MERGE `WHEN |` / `WHEN NOT |` introducer is a keyword-only slot — only
/// `MATCHED` / `NOT` are grammatical. The `ON (...)` condition before the first
/// `WHEN` leaves the cursor in a column phase (`JoinCondition`), so without
/// dedicated handling the slot leaked every joined column. The keyword emission
/// and the column suppression are driven by the same `merge_when_action_keywords`
/// helper so they cannot drift apart.
#[test]
fn merge_when_introducer_is_a_keyword_only_slot() {
    const ON: &str = "MERGE INTO emp e USING dept d ON (e.deptno = d.deptno)";
    let kw = |sql: &str| {
        let ctx = analyze_inline_cursor_sql(sql);
        SqlEditorWidget::collect_expected_keyword_suggestions(
            "",
            &ctx,
            Some(crate::db::DatabaseType::Oracle),
        )
    };
    let suppresses = |sql: &str| {
        let ctx = analyze_inline_cursor_sql(sql);
        let has_prefix = sql.find('|').is_some_and(|i| {
            sql[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
        });
        SqlEditorWidget::cursor_is_at_column_suppressing_keyword_slot(&ctx, has_prefix)
    };
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // `WHEN |` → MATCHED / NOT, and columns suppressed.
    let when = format!("{ON} WHEN |");
    assert_eq!(kw(&when), vec!["MATCHED".to_string(), "NOT".to_string()]);
    assert!(suppresses(&when), "WHEN slot must suppress columns");

    // `WHEN NOT |` → MATCHED, columns suppressed.
    let when_not = format!("{ON} WHEN NOT |");
    assert_eq!(kw(&when_not), vec!["MATCHED".to_string()]);
    assert!(suppresses(&when_not), "WHEN NOT slot must suppress columns");

    // Prefix filtering still works.
    let when_m = format!("{ON} WHEN M|");
    let ctx = analyze_inline_cursor_sql(&when_m);
    assert_eq!(
        SqlEditorWidget::collect_expected_keyword_suggestions("M", &ctx, Some(crate::db::DatabaseType::Oracle)),
        vec!["MATCHED".to_string()]
    );

    // `WHEN MATCHED |` still admits a column expression (`AND <cond>` / `THEN`),
    // so it is NOT a column-suppressing slot.
    let when_matched = format!("{ON} WHEN MATCHED |");
    assert!(!suppresses(&when_matched), "WHEN MATCHED must keep its AND-condition columns");

    // A `CASE WHEN |` value expression is not a MERGE action slot.
    let case_when = "SELECT CASE WHEN | END FROM emp";
    assert!(!has(&kw(case_when), "MATCHED"), "CASE WHEN must not offer MATCHED");
    assert!(!suppresses(case_when), "CASE WHEN must keep its condition columns");
}

/// A PL/SQL value expression — the assignment `:=`, the named-argument `=>`, and
/// an `IF`/`ELSIF`/`WHILE` control condition — admits a variable/function/literal
/// but never a bare table/view/synonym. The General-context base used to dump the
/// whole relation catalog there (`v := |` / `IF | THEN` → `EMP`, `DEPT`). Reuses
/// the same PL/SQL control vocabulary the formatter relies on, and runs through
/// the real `base_suggestions_for_context` apply path. Relations still complete
/// where they are valid (a `FROM` clause, an embedded SQL statement).
#[test]
fn plsql_value_expression_suppresses_relations() {
    fn base_after(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor");
        let sql = sql_with_cursor.replace('|', "");
        let (routine_cache, expanded) =
            SqlEditorWidget::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = SqlEditorWidget::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let deep_ctx = analysis.context.clone();
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let context = SqlEditorWidget::classify_intellisense_context(
            &deep_ctx,
            deep_ctx.statement_tokens.as_ref(),
        );
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string(), "DEPT".to_string()];
        data.set_columns_for_table("EMP", vec!["ENAME".into(), "EMPNO".into()]);
        data.rebuild_indices();
        let column_tables = SqlEditorWidget::resolve_column_tables_for_context(None, &deep_ctx);
        let expr_kw = SqlEditorWidget::expression_keyword_context(
            &deep_ctx,
            &data,
            &column_tables,
            !prefix.is_empty(),
            Some(crate::db::DatabaseType::Oracle),
        );
        SqlEditorWidget::base_suggestions_for_context(
            &mut data,
            &prefix,
            None,
            None,
            false,
            context,
            false,
            Some(crate::db::DatabaseType::Oracle),
            expr_kw,
        )
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // Assignment RHS, named-argument value, and IF/ELSIF/WHILE conditions: no
    // relations.
    for sql in [
        "DECLARE v NUMBER; BEGIN v := |; END;",
        "BEGIN proc(p => |); END;",
        "DECLARE v NUMBER; BEGIN IF | THEN NULL; END IF; END;",
        "DECLARE v NUMBER; BEGIN IF v > | THEN NULL; END IF; END;",
        "DECLARE v NUMBER; BEGIN IF v = 1 THEN NULL; ELSIF | THEN NULL; END IF; END;",
        "DECLARE v NUMBER; BEGIN WHILE | LOOP NULL; END LOOP; END;",
        "DECLARE v NUMBER; BEGIN IF (v OR |) THEN NULL; END IF; END;",
        // Routine call arguments are value positions too.
        "BEGIN dbms_output.put_line(|); END;",
        "BEGIN dbms_output.put_line('x' || |); END;",
        "BEGIN my_proc(a, |); END;",
        // Anywhere in an executable block body: RETURN/RAISE, EXCEPTION handler,
        // a PL/SQL CASE condition, an operator-continued expression, and a bare
        // statement start are all positions where a relation is never valid.
        "CREATE FUNCTION f RETURN NUMBER IS BEGIN RETURN |; END;",
        "DECLARE v NUMBER; BEGIN RETURN |; END;",
        "DECLARE v NUMBER; BEGIN RAISE |; END;",
        "BEGIN NULL; EXCEPTION WHEN | THEN NULL; END;",
        "DECLARE v NUMBER; BEGIN CASE WHEN | THEN NULL; END CASE; END;",
        "DECLARE v NUMBER; BEGIN v := v + |; END;",
        "DECLARE v NUMBER; BEGIN v := CASE WHEN x > | THEN 1 END; END;",
        "DECLARE v NUMBER; BEGIN NULL; | END;",
    ] {
        let s = base_after(sql);
        assert!(!has(&s, "EMP") && !has(&s, "DEPT"), "relations leaked into a PL/SQL value position for `{sql}`: {s:?}");
    }

    // The PL/SQL *declaration* type slot is NOT an executable position: a relation
    // is valid there for `%TYPE`/`%ROWTYPE`, so it must keep relation completion —
    // both the top-level DECLARE section and a routine's `IS` declaration section.
    let s = base_after("DECLARE v | BEGIN NULL; END;");
    assert!(has(&s, "EMP"), "relation wrongly suppressed in a DECLARE type slot: {s:?}");
    let s = base_after("CREATE PROCEDURE p IS v | BEGIN NULL; END;");
    assert!(has(&s, "EMP"), "relation wrongly suppressed in an IS-section type slot: {s:?}");

    // A call whose argument is itself a subquery keeps relation completion for
    // that inner query.
    let s = base_after("BEGIN open_cur(CURSOR(SELECT * FROM e|)); END;");
    assert!(has(&s, "EMP"), "relation wrongly suppressed inside a call's subquery argument: {s:?}");

    // A function name in the same slots is still offered (prefix-driven).
    let s = base_after("DECLARE v NUMBER; BEGIN v := to_ch| ; END;");
    assert!(has(&s, "TO_CHAR"), "function dropped from a PL/SQL value position: {s:?}");
    let s = base_after("DECLARE v NUMBER; BEGIN IF to_ch| THEN NULL; END IF; END;");
    assert!(has(&s, "TO_CHAR"), "function dropped from a PL/SQL condition: {s:?}");

    // Relations still complete where they are valid: a FROM clause, and the IF
    // *body* (an embedded SQL statement after THEN), not just the header.
    let s = base_after("BEGIN SELECT * FROM e| ; END;");
    assert!(has(&s, "EMP"), "relation wrongly suppressed in a FROM position: {s:?}");
    let s = base_after("BEGIN IF 1 = 1 THEN SELECT * FROM e| ; END IF; END;");
    assert!(has(&s, "EMP"), "relation wrongly suppressed in an IF body FROM clause: {s:?}");

    // The same flat-base *keyword* noise (clause/statement keywords, and a stray
    // `WHEN`/`THEN` left over from a closed `END CASE`) must also be dropped at a
    // PL/SQL value-*operand* position — not just relations.
    for (sql, kw) in [
        ("DECLARE v NUMBER; BEGIN v := wh| END;", "WHERE"),
        ("DECLARE v NUMBER; BEGIN v := wh| END;", "WHILE"),
        ("DECLARE v NUMBER; BEGIN v := v + cr| END;", "CREATE"),
        ("DECLARE v NUMBER; BEGIN IF v > wh| THEN NULL; END IF; END;", "WHERE"),
        // A closed `END CASE`/`END IF` must not leave the CASE body keywords
        // grammatical at the following operand (the detector miscounted `END`).
        ("DECLARE v NUMBER; BEGIN CASE WHEN v=1 THEN v:=2; END CASE; v := wh| END;", "WHEN"),
        ("DECLARE v NUMBER; BEGIN CASE v WHEN 1 THEN v:=2; END CASE; v := th| END;", "THEN"),
    ] {
        let s = base_after(sql);
        assert!(!has(&s, kw), "{kw} leaked into a PL/SQL value-operand position for `{sql}`: {s:?}");
    }

    // Operand material survives at a value-operand position: a function, an
    // operand-starting keyword (`CASE`/`CAST`), and — inside a genuinely open
    // `CASE` — the body keywords remain available.
    assert!(has(&base_after("DECLARE v NUMBER; BEGIN v := ca| END;"), "CASE"),
        "CASE starter dropped at a PL/SQL value-operand position");
    assert!(has(&base_after("DECLARE v NUMBER; BEGIN v := CASE WHEN x > ca| THEN 1 END; END;"), "CAST"),
        "operand starter dropped inside an open CASE expression");

    // A *statement start* in a block is NOT a value-operand position: the
    // statement keywords (`IF`/`LOOP`/`RETURN`) stay, never filtered away.
    assert!(has(&base_after("DECLARE v NUMBER; BEGIN NULL; if| END;"), "IF"),
        "IF wrongly filtered at a block statement start");
    assert!(has(&base_after("DECLARE v NUMBER; BEGIN NULL; lo| END;"), "LOOP"),
        "LOOP wrongly filtered at a block statement start");
    assert!(has(&base_after("CREATE FUNCTION f RETURN NUMBER IS BEGIN re| END;"), "RETURN"),
        "RETURN wrongly filtered at a block statement start");
}

/// `cursor_is_inside_unclosed_case` must read `END` the way PL/SQL does: a bare
/// `END` closes a SQL `CASE` expression or a `BEGIN` block, while `END IF`/`END
/// LOOP`/`END CASE` each close their own construct. The naive "every `END`
/// closes a `CASE`, every `CASE` word opens one" count broke twice — the `CASE`
/// in `END CASE` reopened a closed case, and the `END` in an inner `END IF`
/// closed a still-open enclosing case.
#[test]
fn cursor_is_inside_unclosed_case_is_plsql_block_aware() {
    fn inside_case(sql_with_cursor: &str) -> bool {
        let cursor = sql_with_cursor.find('|').expect("cursor marker");
        let sql = sql_with_cursor.replace('|', "");
        let spans = super::query_text::tokenize_sql_spanned(&sql);
        let end = spans.partition_point(|span| span.end <= cursor);
        let tokens: Vec<SqlToken> = spans.into_iter().map(|span| span.token).collect();
        SqlEditorWidget::cursor_is_inside_unclosed_case(&tokens, end)
    }

    // Inside an unclosed CASE (SQL expression or PL/SQL statement).
    for sql in [
        "SELECT CASE WHEN x = 1 th| FROM t",
        "DECLARE v NUMBER; BEGIN CASE WHEN v = 1 THEN v := 2; el| END;",
        "SELECT CASE WHEN x = 1 THEN CASE WHEN y = 2 th| END END FROM t",
        // An inner `END IF` closes only the IF — the enclosing CASE stays open.
        "DECLARE v NUMBER; BEGIN CASE WHEN v = 1 THEN IF v = 2 THEN v := 3; END IF; el| END;",
    ] {
        assert!(inside_case(sql), "cursor should be inside an open CASE for `{sql}`");
    }

    // Outside any open CASE.
    for sql in [
        "SELECT CASE WHEN x = 1 THEN 2 END, c| FROM t",
        // A closed `END CASE` must not be read as a still-open case.
        "DECLARE v NUMBER; BEGIN CASE WHEN v = 1 THEN v := 2; END CASE; v := c| END;",
        // A bare `END IF` (no CASE at all) never opens one.
        "DECLARE v NUMBER; BEGIN IF x THEN NULL; END IF; v := c| END;",
    ] {
        assert!(!inside_case(sql), "cursor should be outside any CASE for `{sql}`");
    }
}

/// `CASE` is shared between a SQL `CASE` *expression* and a PL/SQL `CASE`
/// *statement*, so the executable-block detector must not read a bare SQL `CASE`
/// (no enclosing block) as PL/SQL code. A `CASE` only marks executable code when
/// a real `BEGIN` block (or a PL/SQL-only `IF`/`LOOP`) encloses it.
#[test]
fn plsql_executable_block_detection_excludes_bare_sql_case() {
    fn exec_block(sql_with_cursor: &str) -> bool {
        let cursor = sql_with_cursor.find('|').expect("cursor");
        let sql = sql_with_cursor.replace('|', "");
        let toks = super::query_text::tokenize_sql_spanned(&sql);
        let end = toks.partition_point(|s| s.end <= cursor);
        let tokens: Vec<SqlToken> = toks.into_iter().map(|s| s.token).collect();
        SqlEditorWidget::cursor_in_plsql_executable_block(&tokens, end)
    }

    // A bare SQL CASE expression is not PL/SQL executable code.
    assert!(!exec_block("SELECT CASE WHEN x THEN | END FROM emp"));
    assert!(!exec_block("SELECT * FROM emp WHERE x = (CASE WHEN a THEN |)"));
    // Declarations are not executable (relations stay valid for %TYPE/%ROWTYPE).
    assert!(!exec_block("CREATE PACKAGE p IS v |"));
    assert!(!exec_block("DECLARE v | BEGIN NULL; END;"));

    // Real PL/SQL executable positions — including a CASE *inside* a block.
    assert!(exec_block("BEGIN v := |; END;"));
    assert!(exec_block("BEGIN CASE WHEN | THEN NULL; END CASE; END;"));
    assert!(exec_block("BEGIN IF a THEN NULL; END IF; v := |; END;"));
    assert!(exec_block("BEGIN LOOP NULL; END LOOP; v := |; END;"));
    assert!(exec_block("BEGIN BEGIN v := |; END; END;"));
}

/// A qualified wildcard `t.*` is grammatical only when `t` names a row source in
/// scope. An unresolved qualifier (`x.|` where `x` is not a table/alias/CTE/
/// subquery here) yields no columns, so a lone `*` would be the only — bogus —
/// suggestion; it must be suppressed. A real alias/table/CTE keeps `t.*`.
#[test]
fn qualified_wildcard_only_for_in_scope_row_source() {
    let wc = |sql_with_cursor: &str, qualifier: &str| {
        let cursor = sql_with_cursor.find('|').expect("cursor");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        SqlEditorWidget::collect_clause_wildcard_suggestions(&prefix, Some(qualifier), &ctx)
    };
    let has = |v: &[String], s: &str| v.iter().any(|x| x == s);

    // In-scope row sources keep `*`.
    assert!(has(&wc("SELECT e.| FROM emp e", "e"), "*"));
    assert!(has(&wc("SELECT emp.| FROM emp", "emp"), "*"));
    assert!(has(&wc("SELECT d.| FROM emp e JOIN dept d ON e.deptno = d.deptno", "d"), "*"));
    assert!(has(&wc("WITH c AS (SELECT 1 x FROM dual) SELECT c.| FROM c", "c"), "*"));

    // An unresolved qualifier offers no bogus wildcard.
    assert!(wc("SELECT x.| FROM emp e", "x").is_empty());
    assert!(wc("SELECT zzz.| FROM emp", "zzz").is_empty());
}

/// A bind-variable name slot — the identifier right after a `:` introducer
/// (`WHERE c = :|`, `:b|`, `SELECT :|`) — names a free/session-bind identifier,
/// never a column/relation/`*`. The identifier base must be empty there (session
/// bind names arrive via the local-symbol path).
#[test]
fn bind_variable_name_slot_suppresses_columns() {
    fn base(sql_with_cursor: &str) -> Vec<String> {
        let cursor = sql_with_cursor.find('|').expect("cursor");
        let sql = sql_with_cursor.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql_with_cursor);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&sql, cursor);
        let context = SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".into()];
        data.set_columns_for_table("EMP", vec!["ENAME".into(), "EMPNO".into(), "BONUS".into()]);
        data.rebuild_indices();
        let ct = SqlEditorWidget::resolve_column_tables_for_context(None, &ctx);
        let cs = (!ct.is_empty()).then(|| ct.clone());
        let inc = matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll);
        let kw = SqlEditorWidget::expression_keyword_context(&ctx, &data, &ct, !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle));
        SqlEditorWidget::base_suggestions_for_context(
            &mut data, &prefix, None, cs.as_deref(), inc, context, false,
            Some(crate::db::DatabaseType::Oracle), kw)
    }
    let has = |v: &[String], s: &str| v.iter().any(|x| x.eq_ignore_ascii_case(s));

    // Bind name positions: no columns.
    assert!(base("SELECT * FROM emp WHERE empno = :|").is_empty());
    assert!(base("SELECT * FROM emp WHERE empno = :b|").is_empty(), "bind prefix must not match column BONUS");
    assert!(base("SELECT :| FROM emp").is_empty());

    // A normal operand position still completes columns.
    assert!(has(&base("SELECT * FROM emp WHERE empno = |"), "ENAME"));
}

/// Helper: pure keyword tokens of the base suggestion list for `sql` (cursor at
/// `|`), uppercased, for statement-start filtering assertions.
fn statement_start_base_keywords(sql: &str) -> Vec<String> {
    let cursor = sql.find('|').expect("cursor marker");
    let s = sql.replace('|', "");
    let ctx = analyze_inline_cursor_sql(sql);
    let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&s, cursor);
    let context = SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
    let mut data = IntellisenseData::new();
    let ekc = SqlEditorWidget::expression_keyword_context(
        &ctx, &data, &[], !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle),
    );
    let include_columns = matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll);
    SqlEditorWidget::base_suggestions_for_context(
        &mut data, &prefix, None, None, include_columns, context, false,
        Some(crate::db::DatabaseType::Oracle), ekc,
    )
    .into_iter()
    .filter(|x| !x.ends_with("()") && x.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
    .collect()
}

#[test]
fn statement_start_filters_keyword_dump_to_statement_verbs() {
    // At a fresh statement head — top level or a PL/SQL block — the flat keyword
    // dump is restricted to keywords that can actually open a statement; the rest
    // of the prefix-matched catalog (`SAMPLE`/`SEQUENCE`, `IDENTIFIED`/`INTERSECT`/
    // `INTO`, `WELLFORMED`/`WHERE`/`WINDOW`) is dropped as noise.
    let kw = statement_start_base_keywords;

    // Top-level statement start (buffer start, and after a `;` terminator).
    for sql in ["S|", "SELECT 1 FROM dual; S|"] {
        let s = kw(sql);
        assert_eq!(s, vec!["SAVEPOINT", "SELECT", "SET"], "for `{sql}`");
    }

    // PL/SQL block statement start.
    assert_eq!(kw("BEGIN I|"), vec!["IF", "INSERT"]);
    assert_eq!(kw("BEGIN W|"), vec!["WHILE", "WITH"]);
    assert_eq!(kw("BEGIN NULL; E|"), vec!["END", "EXCEPTION", "EXECUTE"]);
}

#[test]
fn statement_start_gates_plsql_construct_continuations() {
    let kw = statement_start_base_keywords;
    let has = |sql: &str, k: &str| kw(sql).iter().any(|x| x == k);

    // `CASE` selector slot (statement and value, simple and searched): only
    // `WHEN` — never the rest of the `W` catalog.
    assert_eq!(kw("BEGIN CASE W|"), vec!["WHEN"]);
    assert_eq!(kw("BEGIN CASE x W|"), vec!["WHEN"]);
    assert_eq!(kw("BEGIN v := CASE x W|"), vec!["WHEN"]);

    // A *value* `CASE` arm offers only the `WHEN`/`ELSE`/`END` continuations —
    // never the procedural statement keywords valid in a *statement* `CASE`.
    assert_eq!(kw("BEGIN v := CASE WHEN c THEN r E|"), vec!["ELSE", "END"]);
    assert!(!has("BEGIN v := CASE WHEN c THEN r E|", "EXECUTE"));
    // A statement `CASE` arm does admit them.
    assert!(has("BEGIN CASE x WHEN 1 THEN E|", "EXECUTE"));

    // `IF`: `ELSIF`/`ELSE` only before the `ELSE` arm is taken.
    assert!(has("BEGIN IF a THEN b; E|", "ELSE"));
    assert!(has("BEGIN IF a THEN b; E|", "ELSIF"));
    assert!(!has("BEGIN IF a THEN b; ELSE c; E|", "ELSE"));
    assert!(!has("BEGIN IF a THEN b; ELSE c; E|", "ELSIF"));

    // `EXIT`/`CONTINUE` reach an enclosing loop, even across a nested `IF`.
    assert!(has("BEGIN LOOP E|", "EXIT"));
    assert!(has("BEGIN FOR i IN 1..10 LOOP IF x THEN E|", "EXIT"));
    assert!(!has("BEGIN E|", "EXIT"));

    // Exception section: directly after `EXCEPTION` only `WHEN`; `EXCEPTION`
    // itself is offered once (before a block has a handler section), not twice.
    assert_eq!(kw("BEGIN NULL; EXCEPTION W|"), vec!["WHEN"]);
    assert!(has("BEGIN NULL; E|", "EXCEPTION"));

    // An operand position inside a block (after `:=`) is not a statement start —
    // the operand allowlist governs, keeping a value function.
    assert!(statement_start_base_keywords("BEGIN v := to_ch|")
        .iter()
        .all(|k| k != "IF" && k != "INSERT"));
}

#[test]
fn statement_start_drops_object_and_function_noise() {
    // A statement head admits no operand material. A SQL top-level statement
    // never starts with an identifier or a function call; a PL/SQL block
    // statement admits a bare procedure/package call but never a function (its
    // result cannot stand as a statement), a sequence, or a table/view.
    let build = || {
        let mut data = IntellisenseData::new();
        data.tables = vec!["STAFF".to_string()];
        data.procedures = vec!["SYNC_DATA".to_string(), "SEND_MAIL".to_string()];
        data.functions = vec!["SUMMARIZE".to_string()];
        data.sequences = vec!["SEQ_ID".to_string()];
        data.packages = vec!["SCHED_PKG".to_string()];
        data.rebuild_indices();
        data
    };
    let run = |sql: &str| -> Vec<String> {
        let cursor = sql.find('|').unwrap();
        let s = sql.replace('|', "");
        let ctx = analyze_inline_cursor_sql(sql);
        let (prefix, _, _) = crate::ui::intellisense::get_word_at_cursor(&s, cursor);
        let context = SqlEditorWidget::classify_intellisense_context(&ctx, ctx.statement_tokens.as_ref());
        let mut data = build();
        let ekc = SqlEditorWidget::expression_keyword_context(
            &ctx, &data, &[], !prefix.is_empty(), Some(crate::db::DatabaseType::Oracle),
        );
        let include_columns = matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll);
        SqlEditorWidget::base_suggestions_for_context(
            &mut data, &prefix, None, None, include_columns, context, false,
            Some(crate::db::DatabaseType::Oracle), ekc,
        )
    };
    let has = |v: &[String], k: &str| v.iter().any(|x| x.eq_ignore_ascii_case(k));

    // Top-level statement start: only statement verbs; no procedure, sequence,
    // function, or function call.
    let top = run("SE|");
    assert!(has(&top, "SELECT") && has(&top, "SET"));
    for noise in ["SEND_MAIL", "SEQ_ID"] {
        assert!(!has(&top, noise), "`{noise}` leaked into a top-level statement start: {top:?}");
    }

    // PL/SQL block statement start: a callable procedure survives; a function
    // call (built-in `SYS_*()` here) does not.
    let blk = run("BEGIN SY|");
    assert!(has(&blk, "SYNC_DATA"), "procedure call dropped from PL/SQL statement start: {blk:?}");
    assert!(
        !blk.iter().any(|x| x.ends_with("()")),
        "a function call leaked into a PL/SQL statement start: {blk:?}"
    );
}
