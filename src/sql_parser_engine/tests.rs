use super::{
    BlockKind, CreatePlsqlKind, CreateState, EndTokenRole, ExternalClauseState, IfState,
    IfSymbolEvent, LineBoundaryAction, LineLeadingMarker, PendingDo, PendingEnd, PendingEndSuffix,
    RoutineFrame, SemicolonAction, SemicolonPolicy, SlashLineKind, SplitState, SqlParserEngine,
    SymbolRole, TimingPointState, TriggerKind, WithClauseState, WithDeclarationState,
};

#[test]
fn pending_subprogram_begin_counter_does_not_underflow_on_malformed_nested_end() {
    let mut state = SplitState {
        block_stack: vec![BlockKind::AsIs, BlockKind::Declare],
        routine_is_stack: vec![RoutineFrame {
            block_depth: 2,
            semicolon_policy: SemicolonPolicy::ForceSplit,
            external_clause_state: ExternalClauseState::None,
            implicit_language_target_is_quoted: false,
        }],
        pending_end: PendingEnd::End,
        pending_subprogram_begins: 0,
        ..SplitState::default()
    };

    state.resolve_pending_end_on_separator_with_token("");

    assert_eq!(
        state.pending_subprogram_begins, 0,
        "malformed END sequence must not underflow nested subprogram tracking"
    );
    assert_eq!(
        state.block_depth(),
        0,
        "plain END should still close both nested scopes"
    );
}

#[test]
fn semicolon_action_classifies_top_level_split() {
    let state = SplitState::default();
    assert_eq!(
        SemicolonAction::from_state(&state),
        SemicolonAction::SplitTopLevel
    );
}

#[test]
fn semicolon_action_keeps_with_clause_declaration_statement_open() {
    let state = SplitState {
        with_clause_state: WithClauseState::InPlsqlDeclaration(
            WithDeclarationState::AwaitingMainQuery,
        ),
        ..SplitState::default()
    };
    assert_eq!(
        SemicolonAction::from_state(&state),
        SemicolonAction::AppendToCurrent
    );
}

#[test]
fn semicolon_action_detects_forced_routine_split() {
    let mut state = SplitState::default();
    state.block_stack.push(BlockKind::AsIs);
    state.routine_is_stack.push(RoutineFrame {
        block_depth: 1,
        semicolon_policy: SemicolonPolicy::ForceSplit,
        external_clause_state: ExternalClauseState::Confirmed,
        implicit_language_target_is_quoted: false,
    });
    assert_eq!(
        SemicolonAction::from_state(&state),
        SemicolonAction::SplitForcedRoutine
    );
}

#[test]
fn semicolon_action_closes_nested_external_routine_without_split() {
    let mut state = SplitState::default();
    state.block_stack.push(BlockKind::AsIs);
    state.block_stack.push(BlockKind::AsIs);
    state.routine_is_stack.push(RoutineFrame {
        block_depth: 2,
        semicolon_policy: SemicolonPolicy::CloseRoutineBlock,
        external_clause_state: ExternalClauseState::Confirmed,
        implicit_language_target_is_quoted: false,
    });
    assert_eq!(
        SemicolonAction::from_state(&state),
        SemicolonAction::CloseRoutineBlock
    );
}

#[test]
fn semicolon_action_keeps_java_source_statement_open_at_top_level() {
    let state = SplitState {
        create_plsql_kind: CreatePlsqlKind::JavaSource,
        ..SplitState::default()
    };

    assert_eq!(
        SemicolonAction::from_state(&state),
        SemicolonAction::AppendToCurrent
    );
}

#[test]
fn slash_line_kind_classifies_supported_line_forms() {
    let pure = "/";
    let block_comment = "/ /*x*/";
    let line_comment = "/ --x";
    let remark = "/ REM x";

    assert_eq!(
        super::classify_line_leading_slash_marker(pure),
        Some(SlashLineKind::PureTerminator)
    );
    assert_eq!(
        super::classify_line_leading_slash_marker(block_comment),
        Some(SlashLineKind::BlockComment)
    );
    assert_eq!(
        super::classify_line_leading_slash_marker(line_comment),
        Some(SlashLineKind::LineComment)
    );
    assert_eq!(
        super::classify_line_leading_slash_marker(remark),
        Some(SlashLineKind::SqlPlusRemark)
    );
}

#[test]
fn slash_line_kind_treats_block_comment_then_line_comment_as_terminator() {
    let marker = super::classify_line_leading_slash_marker("/ /*x*/ -- trailing");
    assert_eq!(marker, Some(SlashLineKind::PureTerminator));

    let marker = super::classify_line_leading_slash_marker("/ /*x*/ REM trailing");
    assert_eq!(marker, Some(SlashLineKind::PureTerminator));
}

#[test]
fn slash_line_kind_supports_multiple_leading_block_comments() {
    let marker = super::classify_line_leading_slash_marker("/ /*a*/ /*b*/");
    assert_eq!(marker, Some(SlashLineKind::BlockComment));
}

#[test]
fn slash_line_kind_rejects_non_comment_text_after_leading_block_comment() {
    let marker = super::classify_line_leading_slash_marker("/ /*a*/ SELECT 1");
    assert_eq!(marker, None);
}

#[test]
fn slash_line_kind_handles_unterminated_block_comment_without_panic() {
    let result = std::panic::catch_unwind(|| {
        super::classify_line_leading_slash_marker("/ /* unterminated");
    });

    assert!(
        result.is_ok(),
        "unterminated slash-leading block comment should not panic"
    );
}

#[test]
fn slash_line_with_leading_block_comment_and_sql_is_not_consumed_as_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("/ /* keep */ SELECT 99 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("BEGIN"),
        "first statement should keep the PL/SQL block: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "/ /* keep */ SELECT 99 FROM dual".to_string(),
        "line must remain as executable SQL text, not slash terminator"
    );
}

#[test]
fn line_leading_marker_detects_host_and_run_after_leading_block_comment() {
    assert_eq!(
        super::classify_line_leading_marker("/*hint*/ @child.sql"),
        LineLeadingMarker::RunScriptAt
    );
    assert_eq!(
        super::classify_line_leading_marker("/*hint*/ !echo hi"),
        LineLeadingMarker::BangHost
    );
}

#[test]
fn with_function_recovery_splits_before_script_commands_after_leading_block_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("/* keep parser state */ @next_script.sql");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("WITH FUNCTION f"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "/* keep parser state */ @next_script.sql".to_string(),
        "line with leading block comment and script command should stay isolated"
    );
}

#[test]
fn mysql_mode_double_dash_without_whitespace_stays_expression() {
    let mut engine = SqlParserEngine::new();
    engine.set_mysql_mode(true);

    let first_line = engine.process_line_and_take_statements("SELECT 5--2;");
    let second_line = engine.process_line_and_take_statements("SELECT 9;");
    let statements = engine.finalize_and_take_statements();

    assert_eq!(
        first_line,
        vec!["SELECT 5--2".to_string()],
        "MySQL parser mode must not treat `--2` as a line comment: {first_line:?}"
    );
    assert_eq!(
        second_line,
        vec!["SELECT 9".to_string()],
        "subsequent statement should still split normally: {second_line:?}"
    );
    assert!(
        statements.is_empty(),
        "no trailing buffered statements expected after both semicolon-terminated lines: {statements:?}"
    );
}

#[test]
fn mysql_mode_hash_after_backslash_escaped_quote_stays_in_string() {
    let mut engine = SqlParserEngine::new();
    engine.set_mysql_mode(true);

    let first_line = engine.process_line_and_take_statements("SELECT 'abc\\'#still string';");
    let second_line = engine.process_line_and_take_statements("SELECT 2;");
    let statements = engine.finalize_and_take_statements();

    assert_eq!(
        first_line,
        vec!["SELECT 'abc\\'#still string'".to_string()],
        "MySQL parser mode must not treat # after an escaped quote as a line comment: {first_line:?}"
    );
    assert_eq!(
        second_line,
        vec!["SELECT 2".to_string()],
        "subsequent statement should still split normally: {second_line:?}"
    );
    assert!(
        statements.is_empty(),
        "no trailing buffered statements expected after both semicolon-terminated lines: {statements:?}"
    );
}

#[test]
fn with_function_end_label_comma_recovers_before_following_cte_as_clause() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("    FUNCTION fmt_mask (p_txt IN VARCHAR2) RETURN VARCHAR2 IS");
    engine.process_line("    BEGIN");
    engine.process_line("        RETURN p_txt;");
    engine.process_line("    END fmt_mask,");

    assert_eq!(engine.state.block_depth(), 0);
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::InPlsqlDeclaration(WithDeclarationState::AwaitingMainQuery)
    );

    engine.process_line("    base_emp AS (");

    assert_eq!(
        engine.state.block_depth(),
        0,
        "CTE AS must not be reinterpreted as a nested PL/SQL AS/IS block"
    );
    assert_eq!(engine.state.paren_depth(), 1);
}

#[test]
fn line_boundary_action_distinguishes_preserved_and_consumed_slash_lines() {
    let waiting_main_query = SplitState {
        with_clause_state: WithClauseState::InPlsqlDeclaration(
            WithDeclarationState::AwaitingMainQuery,
        ),
        ..SplitState::default()
    };
    let block_comment_marker = LineLeadingMarker::Slash(SlashLineKind::BlockComment);
    let pure_marker = LineLeadingMarker::Slash(SlashLineKind::PureTerminator);

    assert_eq!(
        waiting_main_query.line_boundary_action(block_comment_marker, false),
        LineBoundaryAction::SplitBeforeLine
    );
    assert_eq!(
        waiting_main_query.line_boundary_action(pure_marker, false),
        LineBoundaryAction::SplitBeforeLine
    );

    let forced_external = SplitState {
        block_stack: vec![BlockKind::AsIs],
        routine_is_stack: vec![RoutineFrame {
            block_depth: 1,
            semicolon_policy: SemicolonPolicy::ForceSplit,
            external_clause_state: ExternalClauseState::Confirmed,
            implicit_language_target_is_quoted: false,
        }],
        ..SplitState::default()
    };

    assert_eq!(
        forced_external.line_boundary_action(block_comment_marker, false),
        LineBoundaryAction::SplitAndConsumeLine
    );
    assert_eq!(
        forced_external.line_boundary_action(pure_marker, false),
        LineBoundaryAction::SplitAndConsumeLine
    );
}

#[test]
fn slash_terminator_with_block_comment_is_consumed_after_plsql_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("/ /* keep */");
    engine.process_line("SELECT 53 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("BEGIN"));
    assert_eq!(statements[1], "SELECT 53 FROM dual".to_string());
}

#[test]
fn starts_with_alter_session_skips_inline_comments_between_keywords() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("ALTER -- keep");
    engine.process_line("SESSION");

    assert!(
        engine.starts_with_alter_session(),
        "ALTER header with inline line comment should still be recognized as ALTER SESSION"
    );
}

#[test]
fn starts_with_alter_session_skips_inline_block_comments_between_keywords() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("ALTER /* keep */ SESSION");

    assert!(
        engine.starts_with_alter_session(),
        "ALTER header with inline block comment should still be recognized as ALTER SESSION"
    );
}

#[test]
fn if_symbol_event_classifies_characters() {
    assert_eq!(IfSymbolEvent::from_char(' '), IfSymbolEvent::Whitespace);
    assert_eq!(IfSymbolEvent::from_char('('), IfSymbolEvent::OpenParen);
    assert_eq!(IfSymbolEvent::from_char('A'), IfSymbolEvent::Other);
}

#[test]
fn symbol_role_classifies_semicolon_and_pending_end_separators() {
    assert_eq!(SymbolRole::from_char(';', None), SymbolRole::Semicolon);
    assert_eq!(SymbolRole::from_char('/', Some('*')), SymbolRole::Other);
    assert_eq!(
        SymbolRole::from_char('/', Some('1')),
        SymbolRole::PendingEndSeparator
    );
    assert_eq!(SymbolRole::from_char(')', None), SymbolRole::CloseParen);
    assert!(SymbolRole::from_char(')', None).resolves_pending_end());
    assert!(SymbolRole::from_char('/', Some('1')).resolves_pending_end());
    assert!(!SymbolRole::from_char('(', None).resolves_pending_end());
}

#[test]
fn conditional_compilation_block_does_not_break_plsql_depth_tracking() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION f_cc RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  $IF $$DEBUG $THEN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  $ELSE");
    engine.process_line("    RETURN 2;");
    engine.process_line("  $END");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("$IF $$DEBUG $THEN") && statements[0].contains("$END"),
        "conditional compilation directives should stay inside function body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn standalone_function_with_end_label_splits_cleanly_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION labeled_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END labeled_fn;");
    engine.process_line("/");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END labeled_fn"),
        "function END label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn conditional_compilation_with_unbalanced_branch_tokens_keeps_statement_boundary() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION f_cc_unbalanced RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  $IF $$DEBUG $THEN");
    engine.process_line("    BEGIN");
    engine.process_line("      RETURN 1;");
    engine.process_line("  $ELSE");
    engine.process_line("    RETURN 2;");
    engine.process_line("  $END");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE FUNCTION f_cc_unbalanced"),
        "function body should remain intact in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 3 FROM dual".to_string());
}

#[test]
fn create_function_external_call_spec_without_as_is_splits_from_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_call RETURN NUMBER");
    engine.process_line("  EXTERNAL");
    engine.process_line("  NAME \"ext_call\"");
    engine.process_line("  LANGUAGE C;");
    engine.process_line("SELECT 9 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE C"),
        "external call spec should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 9 FROM dual".to_string());
}

#[test]
fn create_function_external_language_wasm_splits_from_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_wasm RETURN NUMBER");
    engine.process_line("  EXTERNAL");
    engine.process_line("  NAME \"ext_wasm\"");
    engine.process_line("  LANGUAGE WASM;");
    engine.process_line("SELECT 10 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
}

#[test]
fn create_function_external_language_r_splits_from_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_r RETURN NUMBER");
    engine.process_line("  EXTERNAL");
    engine.process_line("  NAME \"ext_r\"");
    engine.process_line("  LANGUAGE R;");
    engine.process_line("SELECT 11 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
}

#[test]
fn create_function_external_language_quoted_target_splits_from_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_js RETURN NUMBER");
    engine.process_line("  EXTERNAL");
    engine.process_line("  NAME \"ext_js\"");
    engine.process_line("  LANGUAGE \"JavaScript\";");
    engine.process_line("SELECT 13 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE \"JavaScript\""),
        "quoted language target should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 13 FROM dual".to_string());
}

#[test]
fn create_function_external_language_q_quote_target_splits_from_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_js_q RETURN NUMBER");
    engine.process_line("  EXTERNAL");
    engine.process_line("  NAME \"ext_js_q\"");
    engine.process_line("  LANGUAGE q'[javascript]';");
    engine.process_line("SELECT 14 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE q'[javascript]'"),
        "q-quoted language target should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 14 FROM dual".to_string());
}

#[test]
fn nested_create_tokens_inside_block_do_not_switch_to_create_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  CREATE FUNCTION ghost RETURN NUMBER;");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("BEGIN"),
        "outer block should remain first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 12 FROM dual".to_string());
}

#[test]
fn create_state_transitions_to_plsql_on_create_or_replace_function() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("OR");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("REPLACE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("FUNCTION");

    assert!(state.in_create_plsql());
    assert_eq!(state.create_state, CreateState::None);
}

#[test]
fn create_state_clears_when_non_plsql_target_follows_create() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("TABLE");

    assert!(!state.in_create_plsql());
    assert_eq!(state.create_state, CreateState::None);
}

#[test]
fn create_state_transitions_to_java_source_on_create_and_compile_java_source() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("OR");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("REPLACE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("AND");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("COMPILE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("JAVA");
    assert_eq!(state.create_state, CreateState::AwaitingJavaTarget);

    state.track_create_plsql("SOURCE");

    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::JavaSource);
    assert_eq!(state.create_state, CreateState::None);
}

#[test]
fn create_state_accepts_noforce_modifier_before_trigger() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("NOFORCE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("TRIGGER");

    assert!(state.in_create_plsql());
    assert_eq!(
        state.create_plsql_kind,
        CreatePlsqlKind::Trigger(TriggerKind::Simple)
    );
}

#[test]
fn create_state_accepts_if_not_exists_before_procedure() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("IF");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("NOT");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("EXISTS");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("PROCEDURE");

    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::Procedure);
}

#[test]
fn create_state_accepts_noresolve_and_debug_modifiers_before_function() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("OR");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("REPLACE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("NORESOLVE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("DEBUG");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("FUNCTION");

    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::Function);
}

#[test]
fn create_or_replace_noresolve_debug_function_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE NORESOLVE DEBUG FUNCTION f_noresolve RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE NORESOLVE DEBUG FUNCTION f_noresolve"),
        "function statement should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn create_state_accepts_immutable_modifier_before_function() {
    let mut state = SplitState::default();

    state.track_create_plsql("CREATE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("OR");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("REPLACE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("IMMUTABLE");
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("FUNCTION");

    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::Function);
}

#[test]
fn create_or_replace_immutable_function_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE IMMUTABLE FUNCTION f_immutable RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 77 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE IMMUTABLE FUNCTION f_immutable"),
        "immutable function statement should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 77 FROM dual".to_string());
}

#[test]
fn create_type_body_member_modifier_is_not_treated_as_new_create_target() {
    let mut state = SplitState {
        create_plsql_kind: CreatePlsqlKind::TypeBody,
        create_state: CreateState::AwaitingObjectType,
        ..SplitState::default()
    };

    state.track_create_plsql("MEMBER");
    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::TypeBody);
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);

    state.track_create_plsql("FUNCTION");
    assert!(state.in_create_plsql());
    assert_eq!(state.create_plsql_kind, CreatePlsqlKind::TypeBody);
    assert_eq!(state.create_state, CreateState::AwaitingObjectType);
}

#[test]
fn create_type_body_member_function_splits_before_trailing_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE BODY t_member AS");
    engine.process_line("  MEMBER FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END f;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TYPE BODY t_member AS"),
        "first statement should preserve type body text: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("MEMBER FUNCTION f RETURN NUMBER IS"),
        "first statement should include member function declarative header: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn package_body_init_section_without_end_label_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_init_no_label AS");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p;");
    engine.process_line("BEGIN");
    engine.process_line("  p;");
    engine.process_line("END;");
    engine.process_line("SELECT 99 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("BEGIN\n  p;\nEND"),
        "package body initialization section should remain in first statement: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 99 FROM dual"));
}

#[test]
fn package_body_init_section_with_quoted_end_label_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_init_quoted AS");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p;");
    engine.process_line("BEGIN");
    engine.process_line("  p;");
    engine.process_line("END \"pkg_init_quoted\";");
    engine.process_line("SELECT 100 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"pkg_init_quoted\""),
        "quoted package body END label should remain in first statement: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 100 FROM dual"));
}

#[test]
fn package_body_init_end_with_keyword_label_is_treated_as_label_not_suffix() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY if AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END IF;");
    engine.process_line("SELECT 7 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END IF"),
        "package body END label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 7 FROM dual".to_string());
}

#[test]
fn package_body_init_end_with_qualified_keyword_label_is_treated_as_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY if AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END owner.if;");
    engine.process_line("SELECT 8 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END owner.if"),
        "qualified package body END label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 8 FROM dual".to_string());
}

#[test]
fn package_body_with_qualified_name_uses_last_segment_for_end_label_matching() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY owner.if AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END owner.if;");
    engine.process_line("SELECT 108 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END owner.if"));
    assert_eq!(statements[1], "SELECT 108 FROM dual".to_string());
}

#[test]
fn package_body_qualified_name_with_block_comment_after_dot_tracks_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY owner./*scope*/pkg_demo AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END owner.pkg_demo;");
    engine.process_line("SELECT 201 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END owner.pkg_demo"),
        "package body END label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 201 FROM dual".to_string());
}

#[test]
fn package_body_quoted_qualified_name_with_line_comment_after_dot_tracks_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"owner\". -- keep");
    engine.process_line("\"pkg_demo\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"owner\".\"pkg_demo\";");
    engine.process_line("SELECT 202 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"owner\".\"pkg_demo\""),
        "quoted package body END label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 202 FROM dual".to_string());
}

#[test]
fn package_body_with_three_part_name_uses_last_segment_for_end_label_matching() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY db.owner.exception IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END db.owner.exception;");
    engine.process_line("SELECT 109 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END db.owner.exception"));
    assert_eq!(statements[1], "SELECT 109 FROM dual".to_string());
}

#[test]
fn package_body_end_with_schema_qualified_label_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_qualified_label AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END owner.pkg_qualified_label;");
    engine.process_line("SELECT 102 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END owner.pkg_qualified_label"),
        "qualified package body end label should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 102 FROM dual".to_string());
}

#[test]
fn package_body_end_with_fully_quoted_qualified_label_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"owner\".\"pkg_q\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"owner\".\"pkg_q\";");
    engine.process_line("SELECT 211 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"owner\".\"pkg_q\""),
        "quoted qualified package body end label should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 211 FROM dual".to_string());
}

#[test]
fn package_body_end_with_mixed_quoted_label_segments_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_mixed_label AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"owner\".pkg_mixed_label;");
    engine.process_line("SELECT 212 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END \"owner\".pkg_mixed_label"));
    assert_eq!(statements[1], "SELECT 212 FROM dual".to_string());
}

#[test]
fn package_body_end_label_followed_by_same_line_select_splits_correctly() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_same_line AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END owner.pkg_same_line; SELECT 212 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END owner.pkg_same_line"),
        "package body END label should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 212 FROM dual".to_string());
}

#[test]
fn declare_begin_state_machine_tracks_pending_begin() {
    let mut state = SplitState::default();

    state.handle_block_openers("DECLARE", EndTokenRole::None);
    assert!(state.has_pending_declare_begin());
    assert_eq!(state.block_depth(), 1);

    state.handle_block_openers("BEGIN", EndTokenRole::None);
    assert!(!state.has_pending_declare_begin());
    assert_eq!(state.block_depth(), 1);
}

#[test]
fn nested_subprogram_as_is_state_machine_resets_after_is() {
    let mut state = SplitState {
        block_stack: vec![BlockKind::Begin],
        ..SplitState::default()
    };

    state.handle_block_openers("PROCEDURE", EndTokenRole::None);
    state.handle_block_openers("IS", EndTokenRole::None);

    assert_eq!(state.block_depth(), 2);
}

#[test]
fn pop_block_of_kind_requires_matching_top_block() {
    let mut state = SplitState {
        block_stack: vec![BlockKind::Begin, BlockKind::Case],
        ..SplitState::default()
    };

    assert!(!state.pop_block_of_kind(BlockKind::Begin));
    assert_eq!(state.block_stack, vec![BlockKind::Begin, BlockKind::Case]);

    assert!(state.pop_block_of_kind(BlockKind::Case));
    assert_eq!(state.block_stack, vec![BlockKind::Begin]);
}

#[test]
fn end_token_role_requires_pending_end_state() {
    assert_eq!(
        EndTokenRole::from_token("CASE", PendingEnd::None, false),
        EndTokenRole::None
    );
}

#[test]
fn end_token_role_maps_suffix_with_compound_trigger_scope() {
    assert_eq!(
        EndTokenRole::from_token("CASE", PendingEnd::End, false).suffix(),
        Some(PendingEndSuffix::Case)
    );
    assert_eq!(
        EndTokenRole::from_token("AFTER", PendingEnd::End, false).suffix(),
        None
    );
    assert_eq!(
        EndTokenRole::from_token("AFTER", PendingEnd::End, true).suffix(),
        Some(PendingEndSuffix::TimingPoint)
    );
}

#[test]
fn end_token_role_reports_matching_suffix() {
    let suffix_role = EndTokenRole::Suffix(PendingEndSuffix::Loop);

    assert!(suffix_role.is_suffix(PendingEndSuffix::Loop));
    assert!(!suffix_role.is_suffix(PendingEndSuffix::If));
    assert!(!EndTokenRole::None.is_suffix(PendingEndSuffix::Case));
}

#[test]
fn pending_end_suffix_parse_covers_supported_keywords() {
    assert_eq!(
        PendingEndSuffix::parse("CASE", false),
        Some(PendingEndSuffix::Case)
    );
    assert_eq!(
        PendingEndSuffix::parse("IF", false),
        Some(PendingEndSuffix::If)
    );
    assert_eq!(
        PendingEndSuffix::parse("LOOP", false),
        Some(PendingEndSuffix::Loop)
    );
    assert_eq!(
        PendingEndSuffix::parse("WHILE", false),
        Some(PendingEndSuffix::While)
    );
    assert_eq!(
        PendingEndSuffix::parse("REPEAT", false),
        Some(PendingEndSuffix::Repeat)
    );
    assert_eq!(
        PendingEndSuffix::parse("FOR", false),
        Some(PendingEndSuffix::For)
    );
}

#[test]
fn pending_end_suffix_parse_scopes_timing_point_keywords() {
    assert_eq!(PendingEndSuffix::parse("BEFORE", false), None);
    assert_eq!(
        PendingEndSuffix::parse("AFTER", true),
        Some(PendingEndSuffix::TimingPoint)
    );
}

#[test]
fn end_timing_point_suffix_clears_pending_timing_point_state() {
    let mut state = SplitState {
        pending_end: PendingEnd::End,
        timing_point_state: TimingPointState::AwaitingAsOrIs,
        block_stack: vec![BlockKind::TimingPoint],
        ..SplitState::default()
    };

    state.handle_pending_end_on_token("AFTER", Some(PendingEndSuffix::TimingPoint));

    assert_eq!(state.pending_end, PendingEnd::None);
    assert_eq!(state.timing_point_state, TimingPointState::None);
    assert!(state.block_stack.is_empty());
}

#[test]
fn semicolon_split_resets_transient_state_at_top_level() {
    let mut engine = SqlParserEngine::new();
    engine.current.push_str("SELECT 1");
    engine.state.pending_end = PendingEnd::End;
    engine.state.pending_do = PendingDo::For {
        armed_at_block_depth: 0,
    };
    engine.state.if_state = IfState::AwaitingThen;
    engine.state.clear_paren_stack();

    engine.process_chars_with_observer(&[';'], &mut |_, _, _, _| {}, &mut |_, _| {});

    assert_eq!(engine.take_statements(), vec!["SELECT 1".to_string()]);
    assert!(engine.current.is_empty());
    assert_eq!(engine.state.pending_end, PendingEnd::None);
    assert_eq!(engine.state.pending_do, PendingDo::None);
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.paren_depth(), 0);
}

#[test]
fn pending_do_does_not_get_overwritten_by_new_candidates() {
    let mut state = SplitState {
        pending_do: PendingDo::While {
            armed_at_block_depth: 0,
        },
        ..SplitState::default()
    };

    state.handle_block_openers("FOR", EndTokenRole::None);
    assert_eq!(
        state.pending_do,
        PendingDo::While {
            armed_at_block_depth: 0
        }
    );

    state.handle_block_openers("DO", EndTokenRole::None);
    assert_eq!(state.block_depth(), 1);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::While));
    assert_eq!(state.pending_do, PendingDo::None);
}

#[test]
fn pending_do_arms_when_no_active_candidate_exists() {
    let mut state = SplitState::default();

    state.handle_block_openers("FOR", EndTokenRole::None);
    assert_eq!(
        state.pending_do,
        PendingDo::For {
            armed_at_block_depth: 0
        }
    );

    state.handle_block_openers("DO", EndTokenRole::None);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::For));
    assert_eq!(state.pending_do, PendingDo::None);
}

#[test]
fn pending_do_requires_matching_block_depth_for_do_resolution() {
    let mut state = SplitState::default();

    state.handle_block_openers("FOR", EndTokenRole::None);
    state.block_stack.push(BlockKind::Begin);
    state.handle_block_openers("DO", EndTokenRole::None);

    assert_eq!(state.block_depth(), 1);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::Begin));
    assert_eq!(state.pending_do, PendingDo::None);
}

#[test]
fn semicolon_split_for_external_routine_resets_transient_state() {
    let mut engine = SqlParserEngine::new();
    engine.current.push_str("LANGUAGE C");
    engine.state.block_stack.push(BlockKind::AsIs);
    engine.state.routine_is_stack.push(RoutineFrame {
        block_depth: 1,
        semicolon_policy: SemicolonPolicy::ForceSplit,
        external_clause_state: ExternalClauseState::Confirmed,
        implicit_language_target_is_quoted: false,
    });
    engine.state.pending_end = PendingEnd::End;
    engine.state.pending_do = PendingDo::While {
        armed_at_block_depth: 1,
    };
    engine.state.if_state = IfState::AwaitingThen;
    engine.state.clear_paren_stack();

    engine.process_chars_with_observer(&[';'], &mut |_, _, _, _| {}, &mut |_, _| {});

    assert_eq!(engine.take_statements(), vec!["LANGUAGE C".to_string()]);
    assert!(engine.current.is_empty());
    assert_eq!(engine.state.block_depth(), 0);
    assert_eq!(engine.state.pending_end, PendingEnd::None);
    assert_eq!(engine.state.pending_do, PendingDo::None);
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.paren_depth(), 0);
}

#[test]
fn close_external_routine_semicolon_only_closes_nested_routine_block() {
    let mut state = SplitState {
        block_stack: vec![BlockKind::AsIs, BlockKind::AsIs],
        pending_subprogram_begins: 1,
        routine_is_stack: vec![RoutineFrame {
            block_depth: 2,
            semicolon_policy: SemicolonPolicy::CloseRoutineBlock,
            external_clause_state: ExternalClauseState::Confirmed,
            implicit_language_target_is_quoted: false,
        }],
        ..SplitState::default()
    };

    state.close_external_routine_on_semicolon();

    assert_eq!(state.block_stack, vec![BlockKind::AsIs]);
    assert_eq!(state.pending_subprogram_begins, 0);
    assert!(state.routine_is_stack.is_empty());
}
#[test]
fn separator_resolution_keeps_create_state() {
    let mut state = SplitState {
        pending_end: PendingEnd::End,
        create_plsql_kind: CreatePlsqlKind::Procedure,
        block_stack: vec![BlockKind::Begin],
        ..SplitState::default()
    };

    state.resolve_pending_end_on_separator();

    assert_eq!(state.pending_end, PendingEnd::None);
    assert_eq!(state.block_depth(), 0);
    assert!(state.in_create_plsql());
}

#[test]
fn terminator_resolution_resets_create_state_at_top_level() {
    let mut state = SplitState {
        pending_end: PendingEnd::End,
        create_plsql_kind: CreatePlsqlKind::Procedure,
        block_stack: vec![BlockKind::Begin],
        ..SplitState::default()
    };

    state.resolve_pending_end_on_terminator();

    assert_eq!(state.pending_end, PendingEnd::None);
    assert_eq!(state.block_depth(), 0);
    assert!(!state.in_create_plsql());
}

#[test]
fn eof_resolution_preserves_with_plsql_declaration_mode() {
    let mut state = SplitState {
        pending_end: PendingEnd::End,
        create_plsql_kind: CreatePlsqlKind::Procedure,
        with_clause_state: WithClauseState::InPlsqlDeclaration(
            WithDeclarationState::AwaitingMainQuery,
        ),
        block_stack: vec![BlockKind::Begin],
        ..SplitState::default()
    };

    state.resolve_pending_end_on_eof();

    assert_eq!(state.pending_end, PendingEnd::None);
    assert_eq!(state.block_depth(), 0);
    assert!(state.in_create_plsql());
    assert!(state.in_with_plsql_declaration());
}

#[test]
fn statement_with_midstream_with_keyword_does_not_enter_with_plsql_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT col WITH FROM t;");

    assert_eq!(
        engine.take_statements(),
        vec!["SELECT col WITH FROM t".to_string()]
    );
    assert!(!engine.state.in_with_plsql_declaration());
}

#[test]
fn with_function_waiting_main_query_recovers_on_new_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("CREATE TABLE t_recover_with_fn (id NUMBER);");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "CREATE TABLE t_recover_with_fn (id NUMBER)".to_string()
    );
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_conn_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("CONN scott/tiger");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "CONN scott/tiger".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_disc_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("DISC");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "DISC".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_run_script_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("@child.sql");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "@child.sql".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_start_script_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("START child.sql");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "START child.sql".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_relative_run_script_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("@@child.sql");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "@@child.sql".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_bang_host_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("! ls");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "first statement should keep only WITH declaration: {}",
        statements[0]
    );
    assert_eq!(statements[1], "! ls".to_string());
    assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
}

#[test]
fn with_function_waiting_main_query_recovers_on_sqlplus_report_statement_heads() {
    for report_command in [
        "TIMING START parser_check",
        "TTITLE LEFT 'SPACE Query'",
        "BTITLE LEFT 'Footer'",
        "REPHEADER PAGE",
        "REPFOOTER OFF",
    ] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("WITH");
        engine.process_line("  FUNCTION f RETURN NUMBER IS");
        engine.process_line("  BEGIN");
        engine.process_line("    RETURN 1;");
        engine.process_line("  END;");
        engine.process_line(report_command);
        engine.process_line("SELECT 2 FROM dual;");

        let statements = engine.finalize_and_take_statements();

        assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
        assert!(
            statements[0].starts_with(
                "WITH
  FUNCTION f RETURN NUMBER IS"
            ),
            "first statement should keep only WITH declaration: {}",
            statements[0]
        );
        assert_eq!(statements[1], report_command.to_string());
        assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
    }
}

#[test]
fn with_function_waiting_main_query_recovers_on_password_command_abbreviations() {
    for password_command in ["PASSWO app_user", "PASSWOR app_user", "PASSWORD app_user"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("WITH");
        engine.process_line("  FUNCTION f RETURN NUMBER IS");
        engine.process_line("  BEGIN");
        engine.process_line("    RETURN 1;");
        engine.process_line("  END;");
        engine.process_line(password_command);
        engine.process_line("SELECT 2 FROM dual;");

        let statements = engine.finalize_and_take_statements();

        assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
        assert!(
            statements[0].starts_with(
                "WITH
  FUNCTION f RETURN NUMBER IS"
            ),
            "first statement should keep only WITH declaration: {}",
            statements[0]
        );
        assert_eq!(statements[1], password_command.to_string());
        assert_eq!(statements[2], "SELECT 2 FROM dual".to_string());
    }
}

#[test]
fn create_view_as_with_function_keeps_statement_open_until_main_select_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE VIEW v_with_fn AS");
    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("SELECT f() AS v FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE VIEW v_with_fn AS"),
        "first statement should preserve CREATE VIEW header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("FUNCTION f RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT f() AS v FROM dual"),
        "first statement should include main SELECT body: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn create_view_as_with_procedure_keeps_statement_open_until_main_select_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE VIEW v_with_proc AS");
    engine.process_line("WITH");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END;");
    engine.process_line("SELECT 1 AS v FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE VIEW v_with_proc AS"),
        "first statement should preserve CREATE VIEW header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("PROCEDURE p IS"),
        "first statement should preserve WITH PROCEDURE declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT 1 AS v FROM dual"),
        "first statement should include main SELECT body: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn with_function_keeps_statement_open_until_main_merge_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION pick_id RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("MERGE INTO target_table t");
    engine.process_line("USING dual d");
    engine.process_line("ON (t.id = pick_id())");
    engine.process_line("WHEN MATCHED THEN UPDATE SET t.val = 'Y';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "expected merge + select split");
    assert!(
        statements[0].starts_with("WITH FUNCTION pick_id RETURN NUMBER IS"),
        "first statement should preserve WITH FUNCTION header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("WHEN MATCHED THEN UPDATE SET t.val = 'Y'"),
        "first statement should include MERGE body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn create_noneditionable_package_body_with_external_library_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE NONEDITIONABLE PACKAGE BODY pkg_ext AS");
    engine.process_line("  FUNCTION ext_call RETURN NUMBER IS");
    engine.process_line("  EXTERNAL LIBRARY extlib LANGUAGE C;");
    engine.process_line("END pkg_ext;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "expected package body + select split");
    assert_eq!(
            statements[0],
            "CREATE OR REPLACE NONEDITIONABLE PACKAGE BODY pkg_ext AS\n  FUNCTION ext_call RETURN NUMBER IS\n  EXTERNAL LIBRARY extlib LANGUAGE C;\nEND pkg_ext".to_string()
        );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn package_body_initialization_begin_end_closes_outer_as_is_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_init AS");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p;");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END pkg_init;");
    engine.process_line("SELECT 42 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PACKAGE BODY pkg_init AS"),
        "first statement should keep package body header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("BEGIN\n  NULL;\nEND pkg_init"),
        "first statement should preserve package initialization block: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 42 FROM dual".to_string());
}

#[test]
fn package_body_initialization_begin_end_closes_outer_is_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_init_is IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END pkg_init_is;");
    engine.process_line("SELECT 77 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("BEGIN\n  NULL;\nEND pkg_init_is"),
        "first statement should preserve package body IS initialization block: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 77 FROM dual".to_string());
}

#[test]
fn package_body_init_end_if_label_does_not_capture_outer_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_end_if AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF done_flag;");
    engine.process_line("END pkg_nested_end_if;");
    engine.process_line("SELECT 88 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END IF done_flag;"),
        "first statement should keep nested END IF label: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_nested_end_if"),
        "first statement should close package body at outer END label: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 88 FROM dual".to_string());
}

#[test]
fn package_body_init_nested_end_loop_label_does_not_capture_outer_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_end_loop AS");
    engine.process_line("BEGIN");
    engine.process_line("  LOOP");
    engine.process_line("    EXIT;");
    engine.process_line("  END LOOP done_loop;");
    engine.process_line("END pkg_nested_end_loop;");
    engine.process_line("SELECT 90 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END LOOP done_loop;"),
        "first statement should keep nested END LOOP label: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_nested_end_loop"),
        "first statement should close package body at outer END label: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 90 FROM dual".to_string());
}

#[test]
fn package_body_init_nested_end_case_label_does_not_capture_outer_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_end_case AS");
    engine.process_line("BEGIN");
    engine.process_line("  CASE");
    engine.process_line("    WHEN 1 = 1 THEN");
    engine.process_line("      NULL;");
    engine.process_line("  END CASE done_case;");
    engine.process_line("END pkg_nested_end_case;");
    engine.process_line("SELECT 91 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END CASE done_case;"),
        "first statement should keep nested END CASE label: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_nested_end_case"),
        "first statement should close package body at outer END label: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 91 FROM dual".to_string());
}

#[test]
fn package_body_init_end_if_with_keyword_label_does_not_open_new_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_end_if_keyword_label AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF LOOP;");
    engine.process_line("END pkg_end_if_keyword_label;");
    engine.process_line("SELECT 101 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END IF LOOP;"),
        "first statement should keep END IF keyword label verbatim: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_end_if_keyword_label"),
        "outer package END should still close correctly: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 101 FROM dual".to_string());
}

#[test]
fn package_body_init_end_loop_with_keyword_label_does_not_open_new_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_end_loop_keyword_label AS");
    engine.process_line("BEGIN");
    engine.process_line("  LOOP");
    engine.process_line("    EXIT;");
    engine.process_line("  END LOOP IF;");
    engine.process_line("END pkg_end_loop_keyword_label;");
    engine.process_line("SELECT 102 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END LOOP IF;"),
        "first statement should keep END LOOP keyword label verbatim: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_end_loop_keyword_label"),
        "outer package END should still close correctly: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 102 FROM dual".to_string());
}

#[test]
fn package_body_init_end_exception_identifier_does_not_capture_outer_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_exception AS");
    engine.process_line("BEGIN");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  EXCEPTION");
    engine.process_line("    WHEN OTHERS THEN");
    engine.process_line("      NULL;");
    engine.process_line("  END inner_block;");
    engine.process_line("END pkg_nested_exception;");
    engine.process_line("SELECT 89 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END inner_block;"),
        "first statement should keep nested END label: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_nested_exception"),
        "first statement should close package body at outer END label: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 89 FROM dual".to_string());
}

#[test]
fn compound_trigger_with_each_row_timing_point_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_each_row");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE EACH ROW IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE EACH ROW;");
    engine.process_line("END;");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END BEFORE EACH ROW"),
        "first statement should preserve EACH ROW timing point closure: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 3 FROM dual"));
}

#[test]
fn compound_trigger_with_statement_timing_point_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_stmt");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 4 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END BEFORE STATEMENT"),
        "first statement should preserve STATEMENT timing point closure: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 4 FROM dual"));
}

#[test]
fn finalize_clears_transient_parser_state_for_reuse() {
    let mut engine = SqlParserEngine::new();
    engine.process_line("FOR i IN 1..10");
    engine.process_line("IF flag");
    // Simulate 3 unmatched open parens via the stack API.
    engine.state.push_open_paren('(');
    engine.state.push_open_paren('(');
    engine.state.push_open_paren('(');

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements, vec!["FOR i IN 1..10\nIF flag".to_string()]);
    assert_eq!(engine.state.pending_do, PendingDo::None);
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.paren_depth(), 0);
}
#[test]
fn type_spec_as_is_follow_state_is_cleared_by_declarative_kind_token() {
    let mut state = SplitState {
        create_plsql_kind: CreatePlsqlKind::TypeSpec,
        ..SplitState::default()
    };

    state.handle_block_openers("AS", EndTokenRole::None);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::AsIs));

    state.handle_block_openers("OBJECT", EndTokenRole::None);
    assert!(state.block_stack.is_empty());
}

#[test]
fn type_body_as_is_does_not_clear_on_type_declarative_kind_tokens() {
    let mut state = SplitState {
        create_plsql_kind: CreatePlsqlKind::TypeBody,
        ..SplitState::default()
    };

    state.handle_block_openers("AS", EndTokenRole::None);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::AsIs));

    state.handle_block_openers("TABLE", EndTokenRole::None);
    assert_eq!(state.block_stack.last(), Some(&BlockKind::AsIs));
}

#[test]
fn compound_trigger_timing_point_uses_dedicated_block_kind() {
    let mut state = SplitState {
        create_plsql_kind: CreatePlsqlKind::Trigger(TriggerKind::Compound),
        timing_point_state: TimingPointState::AwaitingAsOrIs,
        ..SplitState::default()
    };

    state.handle_block_openers("IS", EndTokenRole::None);

    assert_eq!(state.block_stack.last(), Some(&BlockKind::TimingPoint));
    assert_eq!(state.timing_point_state, TimingPointState::None);

    state.pending_end = PendingEnd::End;
    state.handle_pending_end_on_token("AFTER", Some(PendingEndSuffix::TimingPoint));

    assert!(state.block_stack.is_empty());
    assert_eq!(state.pending_end, PendingEnd::None);
}

#[test]
fn compound_trigger_requires_compound_trigger_keyword_pair() {
    let mut state = SplitState {
        create_plsql_kind: CreatePlsqlKind::Trigger(TriggerKind::Simple),
        ..SplitState::default()
    };

    state.handle_block_openers("COMPOUND", EndTokenRole::None);
    assert!(!state.block_stack.contains(&BlockKind::Compound));
    assert_eq!(
        state.create_plsql_kind,
        CreatePlsqlKind::Trigger(TriggerKind::Simple)
    );

    state.handle_block_openers("IS", EndTokenRole::None);
    assert!(!state.block_stack.contains(&BlockKind::Compound));
    assert_eq!(
        state.create_plsql_kind,
        CreatePlsqlKind::Trigger(TriggerKind::Simple)
    );
}

#[test]
fn compound_trigger_header_still_splits_after_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TRIGGER trg_compound"),
        "first statement should preserve COMPOUND TRIGGER body: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn package_with_nested_external_procedure_does_not_split_mid_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg AS");
    engine.process_line("  PROCEDURE ext_proc IS");
    engine.process_line("  EXTERNAL NAME \"ext_proc\" LANGUAGE C;");
    engine.process_line("END pkg;");

    assert_eq!(
            engine.finalize_and_take_statements(),
            vec![
                "CREATE OR REPLACE PACKAGE BODY pkg AS\n  PROCEDURE ext_proc IS\n  EXTERNAL NAME \"ext_proc\" LANGUAGE C;\nEND pkg".to_string()
            ]
        );
}

#[test]
fn package_spec_with_external_procedure_declaration_does_not_split_mid_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE pkg_spec_ext AS");
    engine.process_line("  PROCEDURE ext_proc LANGUAGE C;");
    engine.process_line("END pkg_spec_ext;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PACKAGE pkg_spec_ext AS"),
        "first statement should preserve package specification body: {}",
        statements[0]
    );
    assert!(statements[0].contains("PROCEDURE ext_proc LANGUAGE C;"));
    assert!(statements[0].contains("END pkg_spec_ext"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn package_spec_with_external_name_clause_does_not_split_mid_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE pkg_spec_call AS");
    engine.process_line(r#"  PROCEDURE ext_proc IS EXTERNAL NAME "ext_proc" LANGUAGE C;"#);
    engine.process_line("END pkg_spec_call;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PACKAGE pkg_spec_call AS"),
        "first statement should preserve package specification body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains(r#"PROCEDURE ext_proc IS EXTERNAL NAME "ext_proc" LANGUAGE C;"#),
        "call-spec declaration should stay in package spec statement: {}",
        statements[0]
    );
    assert!(statements[0].contains("END pkg_spec_call"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn package_spec_procedure_language_clause_without_external_keyword_does_not_split_mid_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE pkg_spec_lang AS");
    engine.process_line("  PROCEDURE p IS LANGUAGE C;");
    engine.process_line("END pkg_spec_lang;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("PROCEDURE p IS LANGUAGE C;"));
    assert!(statements[0].contains("END pkg_spec_lang"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn name_language_library_identifiers_do_not_activate_external_clause_policy() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_shadow IS");
    engine.process_line("  name NUMBER := 1;");
    engine.process_line("  language NUMBER := 2;");
    engine.process_line("  library NUMBER := 3;");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow IS"));
    assert!(statements[0].contains("name NUMBER := 1;"));
    assert!(statements[0].contains("language NUMBER := 2;"));
    assert!(statements[0].contains("library NUMBER := 3;"));
    assert!(statements[0].contains("END"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn external_clause_keywords_used_as_identifiers_do_not_force_external_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_shadow_external IS");
    engine.process_line("  external NUMBER := 1;");
    engine.process_line("  parameters NUMBER := 2;");
    engine.process_line("  calling NUMBER := 3;");
    engine.process_line("  with NUMBER := 4;");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow_external IS"));
    assert!(statements[0].contains("external NUMBER := 1;"));
    assert!(statements[0].contains("parameters NUMBER := 2;"));
    assert!(statements[0].contains("calling NUMBER := 3;"));
    assert!(statements[0].contains("with NUMBER := 4;"));
    assert!(statements[0].contains("END"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_identifier_with_language_target_like_datatype_does_not_force_external_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_shadow_c IS");
    engine.process_line("  language c;");
    engine.process_line("  language java;");
    engine.process_line("  language javascript;");
    engine.process_line("  language python;");
    engine.process_line("  language mle;");
    engine.process_line("  marker NUMBER := 1;");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_shadow_c IS"));
    assert!(statements[0].contains("language c;"));
    assert!(statements[0].contains("language java;"));
    assert!(statements[0].contains("language javascript;"));
    assert!(statements[0].contains("language python;"));
    assert!(statements[0].contains("language mle;"));
    assert!(statements[0].contains("marker NUMBER := 1;"));
    assert!(statements[0].contains("END"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_assignment_operator_cancels_implicit_external_detection() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_assign IS");
    engine.process_line("  language := 'C';");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_assign IS"));
    assert!(statements[0].contains("language := 'C';"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_followed_by_line_comment_does_not_cancel_external_clause_detection() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_comment RETURN NUMBER");
    engine.process_line("AS LANGUAGE -- keep parsing as external call spec");
    engine.process_line("C;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_comment RETURN NUMBER"));
    assert!(statements[0].contains("AS LANGUAGE -- keep parsing as external call spec"));
    assert!(statements[0].contains("C;"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_followed_by_block_comment_does_not_cancel_external_clause_detection() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_block_comment RETURN NUMBER");
    engine.process_line("AS LANGUAGE /* keep parsing as external call spec */");
    engine.process_line("C;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0]
        .starts_with("CREATE OR REPLACE FUNCTION ext_lang_block_comment RETURN NUMBER"));
    assert!(statements[0].contains("AS LANGUAGE /* keep parsing as external call spec */"));
    assert!(statements[0].contains("C;"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_followed_by_single_quoted_identifier_literal_does_not_force_external_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_language_literal IS");
    engine.process_line("  language 'C';");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_language_literal IS"));
    assert!(statements[0].contains("language 'C';"));
    assert!(statements[0].contains("END"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_followed_by_double_quoted_identifier_literal_does_not_force_external_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_language_qident IS");
    engine.process_line("  language \"C\";");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_language_qident IS"));
    assert!(statements[0].contains("language \"C\";"));
    assert!(statements[0].contains("END"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_followed_by_quoted_identifier_then_name_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_qident_name RETURN NUMBER");
    engine.process_line("AS LANGUAGE \"C\" NAME 'ext_lang_qident_name';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_lang_qident_name RETURN NUMBER")
    );
    assert!(
        statements[0].contains("AS LANGUAGE \"C\" NAME 'ext_lang_qident_name'"),
        "first statement should keep quoted LANGUAGE target and NAME clause: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn language_followed_by_backtick_identifier_then_name_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_backtick_name RETURN NUMBER");
    engine.process_line("AS LANGUAGE `C` NAME 'ext_lang_backtick_name';");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0]
        .starts_with("CREATE OR REPLACE FUNCTION ext_lang_backtick_name RETURN NUMBER"));
    assert!(
        statements[0].contains("AS LANGUAGE `C` NAME 'ext_lang_backtick_name'"),
        "first statement should keep backtick LANGUAGE target and NAME clause: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn nested_language_identifier_targets_do_not_force_external_split() {
    for target in ["C", "JAVA", "JAVASCRIPT", "PYTHON", "MLE"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE PROCEDURE proc_language_ident IS");
        engine.process_line(&format!("  language {target};"));
        engine.process_line("BEGIN");
        engine.process_line("  NULL;");
        engine.process_line("END;");
        engine.process_line("SELECT 1 FROM dual;");

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for {target}: {statements:?}"
        );
        assert!(
            statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_language_ident IS"),
            "first statement should keep procedure body for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains(&format!("language {target};")),
            "first statement should keep language declaration for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END"),
            "first statement should contain END for {target}: {}",
            statements[0]
        );
        assert!(
            statements[1].starts_with("SELECT 1 FROM dual"),
            "second statement should remain standalone for {target}: {}",
            statements[1]
        );
    }
}

#[test]
fn nested_language_dollar_quoted_targets_do_not_force_external_split() {
    for target in ["$$C$$", "$lang$JAVA$lang$", "$lang$PYTHON$lang$"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE PROCEDURE proc_language_dollar_ident IS");
        engine.process_line(&format!("  language {target};"));
        engine.process_line("BEGIN");
        engine.process_line("  NULL;");
        engine.process_line("END;");
        engine.process_line("SELECT 1 FROM dual;");

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for {target}: {statements:?}"
        );
        assert!(
            statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_language_dollar_ident IS"),
            "first statement should keep procedure body for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains(&format!("language {target};")),
            "first statement should keep language declaration for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END"),
            "first statement should contain END for {target}: {}",
            statements[0]
        );
        assert!(
            statements[1].starts_with("SELECT 1 FROM dual"),
            "second statement should remain standalone for {target}: {}",
            statements[1]
        );
    }
}

#[test]
fn nested_language_dollar_quoted_targets_in_package_body_do_not_close_nested_routine() {
    for target in ["$$C$$", "$lang$JAVASCRIPT$lang$", "$lang$JAVA$lang$"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_language_dollar_ident AS");
        engine.process_line("  PROCEDURE p IS");
        engine.process_line(&format!("    language {target};"));
        engine.process_line("  BEGIN");
        engine.process_line("    NULL;");
        engine.process_line("  END p;");
        engine.process_line("END pkg_language_dollar_ident;");
        engine.process_line("SELECT 1 FROM dual;");

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for nested target {target}: {statements:?}"
        );
        assert!(
            statements[0].contains(&format!("language {target};")),
            "package body should keep nested language declaration for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END p;"),
            "package body should keep nested procedure END for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END pkg_language_dollar_ident"),
            "package body should close normally for {target}: {}",
            statements[0]
        );
        assert!(
            statements[1].starts_with("SELECT 1 FROM dual"),
            "trailing SELECT should split for {target}: {}",
            statements[1]
        );
    }
}

#[test]
fn nested_language_identifier_targets_in_package_body_do_not_close_nested_routine() {
    for target in ["C", "JAVA", "JAVASCRIPT", "PYTHON", "MLE"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_language_ident AS");
        engine.process_line("  PROCEDURE p IS");
        engine.process_line(&format!("    language {target};"));
        engine.process_line("  BEGIN");
        engine.process_line("    NULL;");
        engine.process_line("  END p;");
        engine.process_line("END pkg_language_ident;");
        engine.process_line("SELECT 1 FROM dual;");

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for nested target {target}: {statements:?}"
        );
        assert!(
            statements[0].contains(&format!("language {target};")),
            "package body should keep nested language declaration for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END p;"),
            "package body should keep nested procedure END for {target}: {}",
            statements[0]
        );
        assert!(
            statements[0].contains("END pkg_language_ident"),
            "package body should close normally for {target}: {}",
            statements[0]
        );
        assert!(
            statements[1].starts_with("SELECT 1 FROM dual"),
            "trailing SELECT should split for {target}: {}",
            statements[1]
        );
    }
}

#[test]
fn package_body_nested_language_identifier_declaration_keeps_following_nested_subprograms() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_language_chain AS");
    engine.process_line("  PROCEDURE p1 IS");
    engine.process_line("    language c;");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p1;");
    engine.process_line("  PROCEDURE p2 IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p2;");
    engine.process_line("END pkg_language_chain;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("PROCEDURE p1 IS"));
    assert!(statements[0].contains("language c;"));
    assert!(statements[0].contains("END p1;"));
    assert!(statements[0].contains("PROCEDURE p2 IS"));
    assert!(statements[0].contains("END p2;"));
    assert!(statements[0].contains("END pkg_language_chain"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn nested_language_identifier_declaration_with_following_local_variable_keeps_routine_structure() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_language_locals AS");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("    language c;");
    engine.process_line("    n NUMBER := 1;");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END p;");
    engine.process_line("END pkg_language_locals;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("language c;"));
    assert!(statements[0].contains("n NUMBER := 1;"));
    assert!(statements[0].contains("END p;"));
    assert!(statements[0].contains("END pkg_language_locals"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_comparison_operator_cancels_implicit_external_detection() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE proc_compare IS");
    engine.process_line("  language = 'C';");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE PROCEDURE proc_compare IS"));
    assert!(statements[0].contains("language = 'C';"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_parameters_without_external_keyword_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_params RETURN NUMBER");
    engine.process_line("AS LANGUAGE C PARAMETERS (CONTEXT) ;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C PARAMETERS (CONTEXT)"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_without_external_name_or_parameters_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn external_clause_without_language_target_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_external_only RETURN NUMBER");
    engine.process_line("AS EXTERNAL;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn external_clause_with_credential_keyword_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_cred RETURN NUMBER");
    engine.process_line("AS EXTERNAL CREDENTIAL ext_credential NAME 'ext_cred';");
    engine.process_line("SELECT 101 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("CREDENTIAL ext_credential"),
        "external clause with credential should remain in first statement: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 101 FROM dual"));
}

#[test]
fn language_clause_without_external_keyword_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_only RETURN NUMBER");
    engine.process_line("AS LANGUAGE C NAME 'ext_lang_only';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C NAME 'ext_lang_only'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_single_quoted_target_without_external_keyword_marks_external_routine_split()
{
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_quoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE 'C' NAME 'ext_lang_quoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE 'C' NAME 'ext_lang_quoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_national_single_quoted_target_without_external_keyword_marks_external_routine_split(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_nquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE N'C' NAME 'ext_lang_nquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE N'C' NAME 'ext_lang_nquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_unicode_single_quoted_target_without_external_keyword_marks_external_routine_split(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_uquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE U'C' NAME 'ext_lang_uquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE U'C' NAME 'ext_lang_uquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_unicode_escape_quoted_target_without_external_keyword_marks_external_routine_split(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_uesc RETURN NUMBER");
    engine.process_line("AS LANGUAGE U&'C' NAME 'ext_lang_uesc';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE U&'C' NAME 'ext_lang_uesc'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}
#[test]
fn language_clause_with_q_quoted_target_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_qquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE q'[C]' NAME 'ext_lang_qquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE q'[C]' NAME 'ext_lang_qquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_nq_quoted_target_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_nqquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE nq'[C]' NAME 'ext_lang_nqquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE nq'[C]' NAME 'ext_lang_nqquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_uq_quoted_target_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_uqquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE uq'[C]' NAME 'ext_lang_uqquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE uq'[C]' NAME 'ext_lang_uqquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_binary_single_quoted_target_without_external_keyword_marks_external_routine_split(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_bquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE B'C' NAME 'ext_lang_bquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE B'C' NAME 'ext_lang_bquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_hex_single_quoted_target_without_external_keyword_marks_external_routine_split(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_xquoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE X'C' NAME 'ext_lang_xquoted';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE X'C' NAME 'ext_lang_xquoted'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn apostrophe_cannot_start_q_quote_delimiter_and_does_not_swallow_semicolon_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT q'' FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "SELECT q'' FROM dual".to_string());
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn non_ascii_q_quote_delimiter_is_treated_as_q_quote_and_preserves_semicolon_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT q'가문자열가' FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "SELECT q'가문자열가' FROM dual".to_string());
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn non_ascii_nq_quote_delimiter_is_treated_as_q_quote_and_preserves_semicolon_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT nq'가문자열가' FROM dual;");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "SELECT nq'가문자열가' FROM dual".to_string());
    assert_eq!(statements[1], "SELECT 3 FROM dual".to_string());
}

#[test]
fn package_body_nested_same_delimiter_q_quote_still_splits_on_slash() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_qquote AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("    v_sql CLOB;");
    engine.process_line("  BEGIN");
    engine.process_line("    v_sql := q'[");
    engine.process_line("      UPDATE qt_splitter_boss");
    engine.process_line("         SET payload = q'[dynamic ; payload / still string]'");
    engine.process_line("       WHERE id = :1");
    engine.process_line("    ]';");
    engine.process_line("  END run_me;");
    engine.process_line("END pkg_nested_qquote;");
    engine.process_line("/");
    engine.process_line("SELECT 106 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("CREATE OR REPLACE PACKAGE BODY pkg_nested_qquote AS"));
    assert!(statements[0].contains("payload = q'[dynamic ; payload / still string]'"));
    assert!(statements[0].contains("END pkg_nested_qquote"));
    assert_eq!(statements[1], "SELECT 106 FROM dual".to_string());
}

#[test]
fn oracle_conditional_compilation_flag_does_not_enter_dollar_quote_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  IF $$PLSQL_UNIT IS NOT NULL THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("END;");
    engine.process_line("SELECT 11 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("IF $$PLSQL_UNIT IS NOT NULL THEN"));
    assert!(statements[1].starts_with("SELECT 11 FROM dual"));
}

#[test]
fn dollar_prefixed_numeric_token_does_not_trigger_conditional_compilation_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT $$1$$ FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "SELECT $$1$$ FROM dual".to_string());
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn oracle_conditional_compilation_flag_with_numeric_suffix_does_not_hang_statement_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  IF $$PLSQL_LINE_1 > 0 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("END;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("IF $$PLSQL_LINE_1 > 0 THEN"));
    assert_eq!(statements[1], "SELECT 12 FROM dual".to_string());
}

#[test]
fn language_clause_with_dollar_quoted_target_without_external_keyword_marks_external_routine_split()
{
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_dollar RETURN NUMBER");
    engine.process_line("AS LANGUAGE $lang$C$lang$ NAME 'ext_lang_dollar';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE $lang$C$lang$ NAME 'ext_lang_dollar'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn external_language_javascript_body_starting_with_identifier_inside_dollar_quote_keeps_semicolons()
{
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_js_body RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE JAVASCRIPT NAME $$function run() { return 1; }$$;");
    engine.process_line("SELECT 4 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("$$function run() { return 1; }$$"),
        "dollar-quoted javascript body should remain inside routine definition: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 4 FROM dual"));
}

#[test]
fn implicit_language_javascript_body_starting_with_identifier_inside_dollar_quote_keeps_semicolons()
{
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_js_body2 RETURN NUMBER");
    engine.process_line("AS LANGUAGE JAVASCRIPT NAME $$function run() { return 2; }$$;");
    engine.process_line("SELECT 5 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("$$function run() { return 2; }$$"),
        "implicit language clause should still treat $$..$$ body as one literal: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 5 FROM dual"));
}

#[test]
fn mle_module_clause_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle RETURN NUMBER");
    engine.process_line("AS MLE MODULE ext_mod SIGNATURE 'run(number)';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle RETURN NUMBER"));
    assert!(statements[0].contains("AS MLE MODULE ext_mod SIGNATURE 'run(number)'"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn mle_language_target_with_module_clause_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_lang RETURN NUMBER");
    engine.process_line("AS LANGUAGE JAVASCRIPT MLE MODULE ext_mod SIGNATURE 'run(number)';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_lang RETURN NUMBER"));
    assert!(
        statements[0].contains("AS LANGUAGE JAVASCRIPT MLE MODULE ext_mod SIGNATURE 'run(number)'")
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn external_language_name_clause_without_semicolon_splits_on_slash_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_name_slash RETURN NUMBER");
    engine.process_line("AS LANGUAGE C NAME 'ext_name_slash'");
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_name_slash RETURN NUMBER"));
    assert!(statements[0].contains("AS LANGUAGE C NAME 'ext_name_slash'"));
    assert!(
        statements[1].starts_with("SELECT 1 FROM dual"),
        "slash delimiter line should not leak into next statement: {}",
        statements[1]
    );
}

#[test]
fn mle_module_clause_without_semicolon_splits_on_slash_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_slash RETURN NUMBER");
    engine.process_line("AS MLE MODULE ext_mod SIGNATURE 'run(number)'");
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("CREATE OR REPLACE FUNCTION ext_mle_slash RETURN NUMBER"));
    assert!(statements[0].contains("AS MLE MODULE ext_mod SIGNATURE 'run(number)'"));
    assert!(
        statements[1].starts_with("SELECT 1 FROM dual"),
        "slash delimiter line should not leak into next statement: {}",
        statements[1]
    );
}

#[test]
fn language_clause_with_empty_dollar_quoted_target_still_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_dollar_empty RETURN NUMBER");
    engine.process_line("AS LANGUAGE $$C$$ NAME 'ext_lang_dollar_empty';");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE $$C$$ NAME 'ext_lang_dollar_empty'"));
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn external_language_clause_splits_before_parenthesized_query_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_paren RETURN NUMBER");
    engine.process_line("AS LANGUAGE U'C';");
    engine.process_line("(SELECT ext_lang_paren() AS v FROM dual)");
    engine.process_line("UNION ALL");
    engine.process_line("SELECT 2 AS v FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE U'C'"));
    assert!(statements[1].starts_with("(SELECT ext_lang_paren() AS v FROM dual)"));
}

#[test]
fn language_clause_with_calling_standard_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_calling RETURN NUMBER");
    engine.process_line("AS LANGUAGE C CALLING STANDARD;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("LANGUAGE C CALLING STANDARD"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn simple_trigger_call_body_splits_on_semicolon_without_slash() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_call");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("CALL do_work;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("CALL do_work"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn simple_trigger_when_clause_splits_on_semicolon_without_slash() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_when");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("FOR EACH ROW");
    engine.process_line("WHEN (NEW.id > 0)");
    engine.process_line("CALL do_work;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("WHEN (NEW.id > 0)"));
    assert!(statements[0].contains("CALL do_work"));
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn trigger_referencing_alias_as_does_not_block_call_body_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW AS n OLD AS o");
    engine.process_line("FOR EACH ROW");
    engine.process_line("CALL do_work;");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("REFERENCING NEW AS n OLD AS o"));
    assert!(statements[0].contains("CALL do_work"));
    assert!(statements[1].starts_with("SELECT 3 FROM dual"));
}

#[test]
fn trigger_referencing_alias_is_does_not_block_is_header_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias_is");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW IS n OLD IS o");
    engine.process_line("FOR EACH ROW");
    engine.process_line("IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 5 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("REFERENCING NEW IS n OLD IS o"));
    assert!(statements[0].contains(
        "FOR EACH ROW
IS
BEGIN"
    ));
    assert!(statements[1].starts_with("SELECT 5 FROM dual"));
}

#[test]
fn trigger_header_is_still_opens_simple_trigger_body() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_is_header");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("FOR EACH ROW");
    engine.process_line("IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 4 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TRIGGER trg_is_header"),
        "first statement should preserve trigger header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("FOR EACH ROW\nIS\nBEGIN"),
        "IS header must remain attached to trigger body: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 4 FROM dual"));
}

#[test]
fn language_clause_with_with_context_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_with_context RETURN NUMBER");
    engine.process_line("AS LANGUAGE C WITH CONTEXT;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("LANGUAGE C WITH CONTEXT"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn function_declarative_language_quoted_identifier_does_not_split_before_begin() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION fn_language_ident RETURN NUMBER");
    engine.process_line("IS");
    engine.process_line("  language \"C\";");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE FUNCTION fn_language_ident RETURN NUMBER"),
        "first statement should preserve function body: {}",
        statements[0]
    );
    assert!(statements[0].contains("language \"C\";"));
    assert!(statements[0].contains("BEGIN\n  RETURN 1;\nEND"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn language_clause_with_quoted_target_and_semicolon_splits_before_next_top_level_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_quoted RETURN NUMBER");
    engine.process_line("AS LANGUAGE \"JavaScript\";");
    engine.process_line("SELECT 56 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE \"JavaScript\""),
        "external call spec should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 56 FROM dual".to_string());
}

#[test]
fn language_clause_with_quoted_target_and_semicolon_keeps_following_begin_in_same_function() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION fn_lang_quoted_block RETURN NUMBER");
    engine.process_line("IS");
    engine.process_line("  LANGUAGE \"C\";");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("SELECT 57 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("LANGUAGE \"C\";\nBEGIN"));
    assert_eq!(statements[1], "SELECT 57 FROM dual".to_string());
}

#[test]
fn language_clause_with_future_tokens_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_future RETURN NUMBER");
    engine.process_line("AS LANGUAGE JAVASCRIPT MODULE ext_future_impl;");
    engine.process_line("SELECT 6 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE JAVASCRIPT MODULE ext_future_impl"),
        "first statement should keep future LANGUAGE clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 6 FROM dual"));
}

#[test]
fn language_clause_with_r_without_external_keyword_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_r_implicit RETURN NUMBER");
    engine.process_line("AS LANGUAGE R;");
    engine.process_line("SELECT 15 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("LANGUAGE R"));
    assert_eq!(statements[1], "SELECT 15 FROM dual".to_string());
}

#[test]
fn language_clause_with_wasm_without_external_keyword_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_wasm_implicit RETURN NUMBER");
    engine.process_line("AS LANGUAGE WASM;");
    engine.process_line("SELECT 16 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("LANGUAGE WASM"));
    assert_eq!(statements[1], "SELECT 16 FROM dual".to_string());
}

#[test]
fn package_body_nested_language_clause_with_future_tokens_closes_on_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_future AS");
    engine.process_line("  PROCEDURE p IS LANGUAGE JAVASCRIPT MODULE impl;");
    engine.process_line("END pkg_future;");
    engine.process_line("SELECT 7 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE p IS LANGUAGE JAVASCRIPT MODULE impl;"),
        "nested LANGUAGE clause should stay inside package body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_future"),
        "package body should close normally after nested routine: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 7 FROM dual"));
}

#[test]
fn language_clause_with_language_mle_module_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_language_mle_module RETURN NUMBER");
    engine.process_line("AS LANGUAGE MLE MODULE ext_language_mle_impl;");
    engine.process_line("SELECT 9 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE MLE MODULE ext_language_mle_impl"),
        "first statement should keep LANGUAGE MLE MODULE clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 9 FROM dual"));
}

#[test]
fn language_clause_with_mle_module_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER");
    engine.process_line("AS MLE MODULE ext_mle_impl;");
    engine.process_line("SELECT 8 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE MODULE ext_mle_impl"),
        "first statement should keep MLE MODULE clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 8 FROM dual"));
}

#[test]
fn language_clause_with_mle_signature_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_sig RETURN NUMBER");
    engine.process_line("AS MLE SIGNATURE ext_sig_impl;");
    engine.process_line("SELECT 10 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE SIGNATURE ext_sig_impl"),
        "first statement should keep MLE SIGNATURE clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 10 FROM dual"));
}

#[test]
fn language_clause_with_mle_environment_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER");
    engine.process_line("AS MLE ENV ext_env_impl;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE ENV ext_env_impl"),
        "first statement should keep MLE ENV clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn language_clause_with_mle_imports_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_imports RETURN NUMBER");
    engine.process_line("AS MLE IMPORTS ext_imports_impl;");
    engine.process_line("SELECT 13 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE IMPORTS ext_imports_impl"),
        "first statement should keep MLE IMPORTS clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 13 FROM dual"));
}

#[test]
fn language_clause_with_mle_exports_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_exports RETURN NUMBER");
    engine.process_line("AS MLE EXPORTS ext_exports_impl;");
    engine.process_line("SELECT 14 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE EXPORTS ext_exports_impl"),
        "first statement should keep MLE EXPORTS clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 14 FROM dual"));
}

#[test]
fn language_clause_with_mle_import_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_import RETURN NUMBER");
    engine.process_line("AS MLE IMPORT ext_import_impl;");
    engine.process_line("SELECT 15 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE IMPORT ext_import_impl"),
        "first statement should keep MLE IMPORT clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 15 FROM dual"));
}

#[test]
fn language_clause_with_mle_export_without_external_keyword_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_export RETURN NUMBER");
    engine.process_line("AS MLE EXPORT ext_export_impl;");
    engine.process_line("SELECT 16 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS MLE EXPORT ext_export_impl"),
        "first statement should keep MLE EXPORT clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 16 FROM dual"));
}

#[test]
fn language_clause_with_mle_marker_after_language_target_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_marker RETURN NUMBER");
    engine.process_line("AS LANGUAGE JAVASCRIPT MLE;");
    engine.process_line("SELECT 11 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE JAVASCRIPT MLE"),
        "first statement should keep LANGUAGE ... MLE clause tokens: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 11 FROM dual"));
}

#[test]
fn package_body_nested_language_clause_with_mle_marker_closes_on_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_mle_marker AS");
    engine.process_line("  PROCEDURE p IS LANGUAGE JAVASCRIPT MLE;");
    engine.process_line("END pkg_mle_marker;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE p IS LANGUAGE JAVASCRIPT MLE;"),
        "nested LANGUAGE ... MLE clause should stay in package body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_mle_marker"),
        "package body should close normally after nested routine: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn create_forward_crossedition_trigger_splits_before_trailing_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FORWARD CROSSEDITION TRIGGER trg_forward");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE FORWARD CROSSEDITION TRIGGER"),
        "first statement should preserve trigger header: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn create_reverse_crossedition_trigger_splits_before_trailing_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE REVERSE CROSSEDITION TRIGGER trg_reverse");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE REVERSE CROSSEDITION TRIGGER"),
        "first statement should preserve trigger header: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn type_varying_array_declaration_splits_at_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine
        .process_line("CREATE OR REPLACE TYPE phone_list_t IS VARYING ARRAY(10) OF VARCHAR2(25);");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
        engine.finalize_and_take_statements(),
        vec![
            "CREATE OR REPLACE TYPE phone_list_t IS VARYING ARRAY(10) OF VARCHAR2(25)".to_string(),
            "SELECT 1 FROM dual".to_string(),
        ]
    );
}

#[test]
fn type_enum_declaration_splits_at_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE color_t AS ENUM ('RED', 'GREEN');");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
        engine.finalize_and_take_statements(),
        vec![
            "CREATE OR REPLACE TYPE color_t AS ENUM ('RED', 'GREEN')".to_string(),
            "SELECT 1 FROM dual".to_string(),
        ]
    );
}

#[test]
fn type_range_declaration_splits_at_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE age_t AS RANGE (SUBTYPE = NUMBER);");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
        engine.finalize_and_take_statements(),
        vec![
            "CREATE OR REPLACE TYPE age_t AS RANGE (SUBTYPE = NUMBER)".to_string(),
            "SELECT 1 FROM dual".to_string(),
        ]
    );
}

#[test]
fn type_range_declaration_with_is_keyword_splits_at_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE age_t IS RANGE (SUBTYPE = NUMBER);");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
        engine.finalize_and_take_statements(),
        vec![
            "CREATE OR REPLACE TYPE age_t IS RANGE (SUBTYPE = NUMBER)".to_string(),
            "SELECT 1 FROM dual".to_string(),
        ]
    );
}

#[test]
fn type_declaration_with_unknown_declarative_kind_splits_at_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE t_future AS FUTURE_KIND (");
    engine.process_line("  attr NUMBER");
    engine.process_line(");");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
        engine.finalize_and_take_statements(),
        vec![
            "CREATE OR REPLACE TYPE t_future AS FUTURE_KIND (\n  attr NUMBER\n)".to_string(),
            "SELECT 1 FROM dual".to_string(),
        ]
    );
}

#[test]
fn type_body_local_table_type_declaration_does_not_split_member_body() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE BODY t_local_types AS");
    engine.process_line("  MEMBER PROCEDURE p IS");
    engine.process_line("    TYPE num_tab IS TABLE OF NUMBER;");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END;");
    engine.process_line("END t_local_types;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TYPE BODY t_local_types AS"),
        "first statement should preserve TYPE BODY header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("TYPE num_tab IS TABLE OF NUMBER;"),
        "local TABLE type declaration should remain in TYPE BODY: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END t_local_types"),
        "TYPE BODY should close at final END: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn type_body_local_ref_cursor_type_declaration_does_not_split_member_body() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE BODY t_local_ref AS");
    engine.process_line("  MEMBER PROCEDURE p IS");
    engine.process_line("    TYPE rc_t IS REF CURSOR;");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END;");
    engine.process_line("END t_local_ref;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TYPE BODY t_local_ref AS"),
        "first statement should preserve TYPE BODY header: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("TYPE rc_t IS REF CURSOR;"),
        "local REF CURSOR type declaration should remain in TYPE BODY: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END t_local_ref"),
        "TYPE BODY should close at final END: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn end_with_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END done_label;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2);
    assert!(statements[0].contains("END done_label"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_with_quoted_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"done_label\";");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END \"done_label\""));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn implicit_external_split_clears_routine_boundary_before_next_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_name_first RETURN NUMBER");
    engine.process_line("AS EXTERNAL");
    engine.process_line("NAME \"ext_name_first\" LIBRARY extlib LANGUAGE C;");
    engine.process_line("SELECT 1 FROM dual;");

    assert_eq!(
            engine.finalize_and_take_statements(),
            vec![
                "CREATE OR REPLACE FUNCTION ext_name_first RETURN NUMBER\nAS EXTERNAL\nNAME \"ext_name_first\" LIBRARY extlib LANGUAGE C;".to_string(),
                "SELECT 1 FROM dual".to_string(),
            ]
        );
}

#[test]
fn end_if_with_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF done_flag;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END IF done_flag;"),
        "first statement should include END IF label: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_if_with_quoted_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  END IF \"done_flag\";");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END IF \"done_flag\";"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_loop_with_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  LOOP");
    engine.process_line("    EXIT;");
    engine.process_line("  END LOOP loop_done;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END LOOP loop_done;"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_loop_with_quoted_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  LOOP");
    engine.process_line("    EXIT;");
    engine.process_line("  END LOOP \"loop_done\";");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END LOOP \"loop_done\";"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_case_with_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  CASE");
    engine.process_line("    WHEN 1 = 1 THEN NULL;");
    engine.process_line("  END CASE case_done;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END CASE case_done;"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_case_with_quoted_label_closes_block_and_splits_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  CASE");
    engine.process_line("    WHEN 1 = 1 THEN NULL;");
    engine.process_line("  END CASE \"case_done\";");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END CASE \"case_done\";"));
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_case_with_inner_end_if_label_stays_in_same_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  CASE");
    engine.process_line("    WHEN 1 = 1 THEN");
    engine.process_line("      IF 1 = 1 THEN");
    engine.process_line("        NULL;");
    engine.process_line("      END IF cond_done;");
    engine.process_line("  END CASE case_done;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END IF cond_done;"),
        "END IF label should stay in first statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END CASE case_done;"),
        "END CASE should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn end_if_with_nested_case_label_stays_in_same_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    CASE");
    engine.process_line("      WHEN 1 = 1 THEN NULL;");
    engine.process_line("    END CASE case_done;");
    engine.process_line("  END IF cond_done;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END CASE case_done;"),
        "END CASE label should stay in first statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END IF cond_done;"),
        "END IF should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn trigger_referencing_alias_with_quoted_identifier_does_not_block_body_as_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias_quoted");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW AS \"N\"");
    engine.process_line("FOR EACH ROW");
    engine.process_line("AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("REFERENCING NEW AS \"N\""),
        "first statement should preserve quoted alias clause: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("AS\nBEGIN"),
        "trigger body AS should remain part of trigger statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn trigger_referencing_alias_with_quoted_identifier_does_not_block_body_is_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias_quoted_is");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW IS \"N\"");
    engine.process_line("FOR EACH ROW");
    engine.process_line("IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("REFERENCING NEW IS \"N\""),
        "first statement should preserve quoted alias clause: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("IS\nBEGIN"),
        "trigger body IS should remain part of trigger statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_for_each_row_header_does_not_affect_statement_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_each_row");
    engine.process_line("FOR UPDATE ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE EACH ROW IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE EACH ROW;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "expected trigger + select split");
    assert!(
        statements[0].contains("END BEFORE EACH ROW"),
        "compound trigger body should remain intact: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_nested_subprogram_named_before_does_not_start_new_timing_point() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_nested_before");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("    PROCEDURE before IS");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    END before;");
    engine.process_line("  BEGIN");
    engine.process_line("    before;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE TRIGGER trg_nested_before"),
        "compound trigger should stay in a single statement: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn compound_trigger_nested_subprogram_named_after_keeps_timing_point_balance() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_nested_after");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("    PROCEDURE after IS");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    END after;");
    engine.process_line("  BEGIN");
    engine.process_line("    after;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("  AFTER STATEMENT IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END AFTER STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END BEFORE STATEMENT"),
        "first timing-point END should stay in trigger statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END AFTER STATEMENT"),
        "second timing-point END should stay in trigger statement: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn compound_trigger_nested_labeled_block_named_before_does_not_close_timing_point() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_label_before");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("    <<before>>");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    END before;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END before"),
        "labeled nested block should stay inside trigger body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END BEFORE STATEMENT"),
        "timing-point close should remain attached to compound trigger: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_nested_labeled_block_named_after_does_not_close_timing_point() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_label_after");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  AFTER STATEMENT IS");
    engine.process_line("    <<after>>");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    END after;");
    engine.process_line("  END AFTER STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END after"),
        "labeled nested block should stay inside trigger body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END AFTER STATEMENT"),
        "timing-point close should remain attached to compound trigger: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_nested_labeled_block_named_instead_does_not_close_timing_point() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_label_instead");
    engine.process_line("INSTEAD OF INSERT ON v_orders");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  INSTEAD OF EACH ROW IS");
    engine.process_line("    <<instead>>");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    END instead;");
    engine.process_line("  END INSTEAD OF EACH ROW;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END instead"),
        "labeled nested block should stay inside trigger body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END INSTEAD OF EACH ROW"),
        "timing-point close should remain attached to compound trigger: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_body_identifier_before_followed_by_is_does_not_open_timing_point() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_before_ident");
    engine.process_line("FOR UPDATE ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT IS");
    engine.process_line("  BEGIN");
    engine.process_line("    IF before_value IS NULL THEN");
    engine.process_line("      NULL;");
    engine.process_line("    END IF;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn with_function_followed_by_recursive_with_query_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("WITH r (n) AS (");
    engine.process_line("  SELECT 1 FROM dual");
    engine.process_line("  UNION ALL");
    engine.process_line("  SELECT n + 1 FROM r WHERE n < 3");
    engine.process_line(")");
    engine.process_line("SELECT * FROM r;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH r (n) AS"),
        "recursive WITH should stay attached to WITH FUNCTION statement: {}",
        statements[0]
    );
    assert!(
        statements[0].ends_with("SELECT * FROM r"),
        "main query should remain attached: {}",
        statements[0]
    );
}

#[test]
fn with_function_followed_by_non_recursive_with_query_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("WITH cte AS (SELECT f() AS v FROM dual)");
    engine.process_line("SELECT v FROM cte;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH cte AS"),
        "CTE WITH should be treated as a valid main query head: {}",
        statements[0]
    );
}

#[test]
fn with_function_declaration_with_end_label_and_trailing_ctes_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("    FUNCTION calc_depth (p_id NUMBER) RETURN NUMBER IS v_depth NUMBER;");
    engine.process_line("BEGIN");
    engine.process_line("    SELECT MAX (LEVEL)");
    engine.process_line("    INTO v_depth");
    engine.process_line("    FROM org_tree");
    engine.process_line("    START WITH parent_id IS NULL");
    engine.process_line("    CONNECT BY PRIOR node_id = parent_id;");
    engine.process_line("    RETURN v_depth;");
    engine.process_line("END calc_depth;");
    engine.process_line("");
    engine.process_line("recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS (");
    engine.process_line("    SELECT node_id,");
    engine.process_line("        parent_id,");
    engine.process_line("        node_name,");
    engine.process_line("        1 AS DEPTH,");
    engine.process_line("        CAST (node_name AS VARCHAR2 (4000)) AS PATH");
    engine.process_line("    FROM org_tree");
    engine.process_line("    WHERE parent_id IS NULL");
    engine.process_line("    UNION ALL");
    engine.process_line("    SELECT t.node_id,");
    engine.process_line("        t.parent_id,");
    engine.process_line("        t.node_name,");
    engine.process_line("        rt.DEPTH + 1,");
    engine.process_line("        rt.PATH || ' > ' || t.node_name");
    engine.process_line("    FROM org_tree t");
    engine.process_line("    JOIN recursive_tree rt");
    engine.process_line("        ON t.parent_id = rt.node_id");
    engine.process_line("    WHERE rt.DEPTH < calc_depth (t.node_id)");
    engine.process_line("),");
    engine.process_line("    aggregated AS (");
    engine.process_line("        SELECT parent_id,");
    engine.process_line("            COUNT (*) AS child_count,");
    engine.process_line("            MAX (DEPTH) AS max_depth,");
    engine.process_line(
        "            LISTAGG (node_name, ', ') WITHIN GROUP (ORDER BY node_name) AS children",
    );
    engine.process_line("        FROM recursive_tree");
    engine.process_line("        WHERE DEPTH > 1");
    engine.process_line("        GROUP BY parent_id");
    engine.process_line("    )");
    engine.process_line("SELECT rt.*,");
    engine.process_line("    a.child_count,");
    engine.process_line("    a.max_depth,");
    engine.process_line("    a.children");
    engine.process_line("FROM recursive_tree rt");
    engine.process_line("LEFT JOIN aggregated a");
    engine.process_line("    ON rt.node_id = a.parent_id");
    engine.process_line("ORDER BY rt.PATH;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END calc_depth;"),
        "function end label should remain attached: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("recursive_tree (node_id, parent_id, node_name, DEPTH, PATH) AS"),
        "CTE header should remain attached after WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("aggregated AS ("),
        "trailing CTE should remain attached after WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("ORDER BY rt.PATH"),
        "main SELECT should remain attached after trailing CTEs: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn non_plsql_with_clause_resets_pending_with_declaration_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE VIEW v_read_only AS");
    engine.process_line("SELECT * FROM dual WITH READ ONLY;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH READ ONLY"),
        "first statement should preserve trailing WITH READ ONLY clause: {}",
        statements[0]
    );
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::None,
        "non-PL/SQL WITH clauses should not leave declaration tracking armed"
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn non_plsql_with_check_option_clause_resets_pending_with_declaration_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE VIEW v_checked AS");
    engine.process_line("SELECT * FROM dual WITH CHECK OPTION;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH CHECK OPTION"),
        "first statement should preserve WITH CHECK OPTION clause: {}",
        statements[0]
    );
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::None,
        "WITH CHECK OPTION should not leave declaration tracking armed"
    );
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn non_plsql_with_rowid_clause_resets_pending_with_declaration_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE VIEW v_rowid AS");
    engine.process_line("SELECT rowid rid, t.* FROM t WITH ROWID;");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH ROWID"),
        "first statement should preserve WITH ROWID clause: {}",
        statements[0]
    );
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::None,
        "WITH ROWID should not leave declaration tracking armed"
    );
    assert_eq!(statements[1], "SELECT 3 FROM dual".to_string());
}

#[test]
fn non_plsql_with_clause_variants_reset_pending_with_declaration_mode() {
    for (suffix, marker) in [("WITH NO DATA", "NO DATA"), ("WITH TIES", "TIES")] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE VIEW v_non_plsql_clause AS");
        engine.process_line(&format!("SELECT 1 AS v FROM dual {suffix};"));
        engine.process_line("SELECT 4 FROM dual;");

        let statements = engine.finalize_and_take_statements();

        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for {suffix}: {statements:?}"
        );
        assert!(
            statements[0].contains(marker),
            "first statement should preserve trailing {marker} clause: {}",
            statements[0]
        );
        assert_eq!(
            engine.state.with_clause_state,
            WithClauseState::None,
            "{marker} should not leave declaration tracking armed"
        );
        assert_eq!(statements[1], "SELECT 4 FROM dual".to_string());
    }
}

#[test]
fn materialized_view_log_with_sequence_resets_pending_with_declaration_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE MATERIALIZED VIEW LOG ON mv_test WITH SEQUENCE;");
    engine.process_line("SELECT 9 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH SEQUENCE"),
        "first statement should preserve WITH SEQUENCE clause: {}",
        statements[0]
    );
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::None,
        "WITH SEQUENCE should not leave declaration tracking armed"
    );
    assert_eq!(statements[1], "SELECT 9 FROM dual".to_string());
}

#[test]
fn materialized_view_log_with_commit_scn_resets_pending_with_declaration_mode() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE MATERIALIZED VIEW LOG ON mv_test WITH COMMIT SCN;");
    engine.process_line("SELECT 10 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("WITH COMMIT SCN"),
        "first statement should preserve WITH COMMIT SCN clause: {}",
        statements[0]
    );
    assert_eq!(
        engine.state.with_clause_state,
        WithClauseState::None,
        "WITH COMMIT SCN should not leave declaration tracking armed"
    );
    assert_eq!(statements[1], "SELECT 10 FROM dual".to_string());
}

#[test]
fn with_package_declaration_keeps_main_query_attached_until_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  PACKAGE pkg_demo AS");
    engine.process_line("    FUNCTION f RETURN NUMBER;");
    engine.process_line("  END pkg_demo;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  PACKAGE pkg_demo AS"
        ),
        "WITH PACKAGE declaration must stay attached to the same top-level statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT 1 FROM dual"),
        "main query should remain attached after WITH PACKAGE declarations: {}",
        statements[0]
    );
}

#[test]
fn with_type_declaration_keeps_main_query_attached_until_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  TYPE t_num IS TABLE OF NUMBER;");
    engine.process_line("SELECT * FROM TABLE(t_num(1, 2));");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  TYPE t_num IS TABLE OF NUMBER;"
        ),
        "WITH TYPE declaration must stay attached to the same top-level statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT * FROM TABLE(t_num(1, 2))"),
        "main query should remain attached after WITH TYPE declarations: {}",
        statements[0]
    );
}

#[test]
fn with_clause_multiple_plsql_declarations_keep_main_query_attached() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("  PROCEDURE p IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END;");
    engine.process_line("SELECT f() FROM dual;");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("FUNCTION f RETURN NUMBER IS"),
        "first statement should contain WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("PROCEDURE p IS"),
        "first statement should contain WITH PROCEDURE declaration: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT f() FROM dual"),
        "first statement should include the main query: {}",
        statements[0]
    );
    assert!(statements[1].starts_with("SELECT 2 FROM dual"));
}

#[test]
fn with_function_supports_all_oracle_main_query_heads() {
    let cases = [
        (
            "SELECT",
            vec!["SELECT f() FROM dual;", "SELECT 2 FROM dual;"],
        ),
        (
            "INSERT",
            vec![
                "INSERT INTO t_result(v) VALUES (f());",
                "SELECT 3 FROM dual;",
            ],
        ),
        (
            "UPDATE",
            vec!["UPDATE t_result SET v = f();", "SELECT 4 FROM dual;"],
        ),
        (
            "DELETE",
            vec!["DELETE FROM t_result WHERE v = f();", "SELECT 5 FROM dual;"],
        ),
        (
            "MERGE",
            vec![
                "MERGE INTO t_result d USING (SELECT f() AS v FROM dual) s ON (d.v = s.v)",
                "WHEN MATCHED THEN UPDATE SET d.v = s.v",
                "WHEN NOT MATCHED THEN INSERT (v) VALUES (s.v);",
                "SELECT 6 FROM dual;",
            ],
        ),
        ("VALUES", vec!["VALUES (f());", "SELECT 7 FROM dual;"]),
        (
            "TABLE",
            vec!["TABLE(sys.odcinumberlist(f()));", "SELECT 8 FROM dual;"],
        ),
        ("CALL", vec!["CALL consume_fn(f());", "SELECT 9 FROM dual;"]),
    ];

    for (head, body_lines) in cases {
        let mut engine = SqlParserEngine::new();
        engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
        engine.process_line("BEGIN");
        engine.process_line("  RETURN 1;");
        engine.process_line("END;");

        for line in body_lines {
            engine.process_line(line);
        }

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
            statements.len(),
            2,
            "{head} main query head should keep WITH FUNCTION block attached: {statements:?}"
        );
        assert!(
            statements[0].contains("WITH FUNCTION f RETURN NUMBER IS"),
            "first statement should include WITH FUNCTION declaration for {head}: {}",
            statements[0]
        );
    }
}

#[test]
fn wrapped_create_splits_on_sqlplus_slash_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE wrapped_pkg wrapped");
    engine.process_line("a000000");
    engine.process_line("1");
    engine.process_line("abcd");
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PACKAGE wrapped_pkg wrapped"),
        "first statement should preserve wrapped DDL header: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn wrapped_create_recovers_on_following_statement_head_without_slash() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped");
    engine.process_line("a000000");
    engine.process_line("abcd");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped"),
        "first statement should preserve wrapped DDL header: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn wrapped_create_recovers_on_following_statement_head_with_leading_block_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped");
    engine.process_line("a000000");
    engine.process_line("abcd");
    engine.process_line("/* keep */ SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped"),
        "first statement should preserve wrapped DDL header: {}",
        statements[0]
    );
    assert_eq!(statements[1], "/* keep */ SELECT 1 FROM dual".to_string());
}

#[test]
fn wrapped_create_recovers_on_following_statement_head_with_leading_line_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped");
    engine.process_line("a000000");
    engine.process_line("abcd");
    engine.process_line("-- keep");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("CREATE OR REPLACE PROCEDURE wrapped_proc wrapped"),
        "first statement should preserve wrapped DDL header: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn with_function_followed_by_multitable_insert_all_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("INSERT ALL");
    engine.process_line("  INTO t_result(v) VALUES (f())");
    engine.process_line("  INTO t_audit(v) VALUES (f() + 1)");
    engine.process_line("SELECT f() FROM dual;");
    engine.process_line("SELECT 10 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements.len(),
        2,
        "multitable INSERT ALL main query should stay attached to WITH FUNCTION: {statements:?}"
    );
    assert!(
        statements[0].contains("INSERT ALL"),
        "first statement should keep INSERT ALL body attached: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SELECT f() FROM dual"),
        "first statement should include INSERT ALL driving SELECT: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SELECT 10 FROM dual"),
        "second statement should preserve trailing SELECT: {}",
        statements[1]
    );
}

#[test]
fn with_function_recovery_splits_before_non_main_query_statement_heads() {
    let cases = [
        (
            "CREATE TABLE",
            vec![
                "CREATE TABLE wf_recovery_t (id NUMBER);",
                "SELECT 42 FROM dual;",
            ],
        ),
        (
            "ALTER SESSION",
            vec![
                "ALTER SESSION SET NLS_DATE_FORMAT = ''YYYY-MM-DD'';",
                "SELECT 43 FROM dual;",
            ],
        ),
        ("AUDIT", vec!["AUDIT SESSION;", "SELECT 44 FROM dual;"]),
    ];

    for (head, lines) in cases {
        let mut engine = SqlParserEngine::new();

        engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
        engine.process_line("BEGIN");
        engine.process_line("  RETURN 1;");
        engine.process_line("END;");

        for line in lines {
            engine.process_line(line);
        }

        let statements = engine.finalize_and_take_statements();
        assert_eq!(
                statements.len(),
                3,
                "{head} should be parsed as a standalone statement after WITH FUNCTION recovery: {statements:?}"
            );
        assert!(
            statements[0].contains("WITH FUNCTION f RETURN NUMBER IS"),
            "first statement should remain the completed WITH FUNCTION declaration for {head}: {}",
            statements[0]
        );
        assert!(
            statements[1].starts_with(head),
            "second statement should start with {head}: {}",
            statements[1]
        );
        assert!(
            statements[2].starts_with("SELECT"),
            "third statement should preserve trailing SELECT after {head}: {}",
            statements[2]
        );
    }
}

#[test]
fn compound_trigger_instead_of_each_row_section_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_instead");
    engine.process_line("INSTEAD OF INSERT ON v_orders");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  INSTEAD OF EACH ROW IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END INSTEAD OF EACH ROW;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END INSTEAD OF EACH ROW"),
        "compound trigger timing-point END must stay inside trigger body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn compound_trigger_after_statement_section_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_after_stmt");
    engine.process_line("FOR UPDATE ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  AFTER STATEMENT IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END AFTER STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 7 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END AFTER STATEMENT"),
        "compound trigger statement timing-point END must stay inside trigger body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 7 FROM dual".to_string());
}

#[test]
fn compound_trigger_timing_point_without_is_still_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_no_is");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE STATEMENT");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE STATEMENT;");
    engine.process_line("END;");
    engine.process_line("SELECT 9 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END BEFORE STATEMENT"),
        "timing-point END without IS must stay inside trigger body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 9 FROM dual".to_string());
}

#[test]
fn compound_trigger_timing_point_with_declare_section_splits_on_outer_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_decl");
    engine.process_line("FOR INSERT ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE EACH ROW IS");
    engine.process_line("    DECLARE");
    engine.process_line("      v_local NUMBER := 1;");
    engine.process_line("    BEGIN");
    engine.process_line("      :NEW.id := v_local;");
    engine.process_line("    END BEFORE EACH ROW;");
    engine.process_line("END;");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("DECLARE\n      v_local NUMBER := 1;"),
        "timing-point declare section should stay inside compound trigger: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END BEFORE EACH ROW"),
        "timing-point END BEFORE should stay inside trigger statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 3 FROM dual".to_string());
}

#[test]
fn compound_trigger_timing_point_end_with_label_stays_in_trigger_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_compound_label");
    engine.process_line("FOR UPDATE ON t");
    engine.process_line("COMPOUND TRIGGER");
    engine.process_line("  BEFORE EACH ROW IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END BEFORE EACH ROW tp_done;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END BEFORE EACH ROW tp_done;"),
        "timing-point END label should stay in compound trigger statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn case_expression_followed_by_for_update_keeps_same_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  SELECT CASE WHEN status = 'READY' THEN id ELSE 0 END");
    engine.process_line("    INTO v_id");
    engine.process_line("    FROM jobs");
    engine.process_line("    FOR UPDATE SKIP LOCKED;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0]
            .contains("END\n    INTO v_id\n    FROM jobs\n    FOR UPDATE SKIP LOCKED;\nEND"),
        "FOR UPDATE clause should remain in the same PL/SQL block after CASE END: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn external_language_parameters_without_semicolon_splits_on_slash() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_params RETURN NUMBER");
    engine.process_line("AS LANGUAGE C PARAMETERS (CONTEXT)");
    engine.process_line("/");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C PARAMETERS (CONTEXT)"),
        "call specification should stay in routine statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SELECT 2 FROM dual"),
        "trailing query should remain standalone after slash delimiter: {}",
        statements[1]
    );
}

#[test]
fn aggregate_using_clause_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_agg RETURN NUMBER");
    engine.process_line("AS AGGREGATE USING ext_agg_impl;");
    engine.process_line("SELECT 11 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS AGGREGATE USING ext_agg_impl"));
    assert!(statements[1].starts_with("SELECT 11 FROM dual"));
}

#[test]
fn pipelined_using_clause_without_external_keyword_marks_external_routine_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_pipe RETURN sys.odcinumberlist");
    engine.process_line("AS PIPELINED USING ext_pipe_impl;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS PIPELINED USING ext_pipe_impl"));
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn sql_macro_call_spec_without_external_keyword_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_macro RETURN VARCHAR2");
    engine.process_line("AS SQL_MACRO;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS SQL_MACRO"));
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn package_nested_sql_macro_call_spec_closes_nested_function_block_on_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_sql_macro AS");
    engine.process_line("  FUNCTION f RETURN VARCHAR2");
    engine.process_line("  IS SQL_MACRO;");
    engine.process_line("END pkg_sql_macro;");
    engine.process_line("SELECT 12 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("FUNCTION f RETURN VARCHAR2"));
    assert!(statements[0].contains("IS SQL_MACRO"));
    assert!(statements[0].contains("END pkg_sql_macro"));
    assert!(statements[1].starts_with("SELECT 12 FROM dual"));
}

#[test]
fn external_language_without_target_but_clause_keywords_still_splits() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE PARAMETERS('x') NAME 'ext_fn';");
    engine.process_line("SELECT 13 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL LANGUAGE PARAMETERS('x') NAME 'ext_fn'"),
        "external call spec should stay in first statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SELECT 13 FROM dual"),
        "SELECT should be split into next statement: {}",
        statements[1]
    );
}

#[test]
fn external_language_without_target_still_splits_at_top_level() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_missing_target RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE;");
    engine.process_line("SELECT 13 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL LANGUAGE"),
        "external call spec should stay in first statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SELECT 13 FROM dual"),
        "SELECT should be split into next statement: {}",
        statements[1]
    );
}

#[test]
fn package_nested_external_without_language_target_closes_on_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_ext_missing_target AS");
    engine.process_line("  PROCEDURE p IS EXTERNAL LANGUAGE PARAMETERS('x') NAME 'p';");
    engine.process_line("END pkg_ext_missing_target;");
    engine.process_line("SELECT 14 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE p IS EXTERNAL LANGUAGE PARAMETERS('x') NAME 'p'"),
        "nested external routine should remain inside package body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_ext_missing_target"),
        "package body END should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 14 FROM dual".to_string());
}

#[test]
fn package_nested_external_language_without_target_closes_on_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_ext_missing_lang_target AS");
    engine.process_line("  PROCEDURE p IS EXTERNAL LANGUAGE;");
    engine.process_line("END pkg_ext_missing_lang_target;");
    engine.process_line("SELECT 14 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE p IS EXTERNAL LANGUAGE"),
        "nested external routine should remain inside package body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_ext_missing_lang_target"),
        "package body END should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 14 FROM dual".to_string());
}

#[test]
fn external_language_clause_splits_before_trailing_line_comment_and_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("-- next statement comment");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        !statements[0].contains("next statement comment"),
        "line comment after external routine should belong to next statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("-- next statement comment\nSELECT 1 FROM dual"),
        "line comment should stay with the following statement: {}",
        statements[1]
    );
}

#[test]
fn with_function_recovers_to_rem_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("REM trailing sqlplus comment");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "REM trailing sqlplus comment".to_string(),
        "REM command should be auto-terminated as standalone statement: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT should remain standalone after REM command split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_to_remark_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH PROCEDURE local_proc IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END local_proc;");
    engine.process_line("REMARK trailing sqlplus comment");
    engine.process_line("SELECT 13 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_proc"),
        "first statement should keep WITH PROCEDURE declaration: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "REMARK trailing sqlplus comment".to_string(),
        "REMARK command should be auto-terminated as standalone statement: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 13 FROM dual"),
        "SELECT should remain standalone after REMARK command split: {}",
        statements[2]
    );
}

#[test]
fn sqlplus_connect_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CONNECT scott/tiger");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "CONNECT scott/tiger".to_string());
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn sqlplus_start_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("START child.sql");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "START child.sql".to_string());
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn bare_start_line_is_not_misclassified_as_sqlplus_start_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT employee_id");
    engine.process_line("FROM employees");
    engine.process_line("START");
    engine.process_line("WITH manager_id IS NULL");
    engine.process_line("CONNECT BY PRIOR employee_id = manager_id;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains(
            "START
WITH manager_id IS NULL"
        ),
        "multi-line START WITH clause should remain in the SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn bare_connect_line_is_not_misclassified_as_sqlplus_connect_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT employee_id");
    engine.process_line("FROM employees");
    engine.process_line("START WITH manager_id IS NULL");
    engine.process_line("CONNECT");
    engine.process_line("BY PRIOR employee_id = manager_id;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains(
            "CONNECT
BY PRIOR employee_id = manager_id"
        ),
        "multi-line CONNECT BY clause should remain in the SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn oracle_select_identifier_prompt_is_not_misclassified_as_sqlplus_prompt_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT");
    engine.process_line("  PROMPT");
    engine.process_line("FROM tool_words;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROMPT"),
        "column identifier should remain in SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn oracle_start_with_clause_is_not_misclassified_as_sqlplus_start_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT employee_id");
    engine.process_line("FROM employees");
    engine.process_line("START WITH manager_id IS NULL");
    engine.process_line("CONNECT BY PRIOR employee_id = manager_id;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("START WITH manager_id IS NULL"),
        "hierarchical START WITH clause should remain in the SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn oracle_start_with_clause_with_inline_comment_is_not_misclassified_as_sqlplus_start_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT employee_id");
    engine.process_line("FROM employees");
    engine.process_line("START /*tree root*/ WITH manager_id IS NULL");
    engine.process_line("CONNECT BY PRIOR employee_id = manager_id;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("START /*tree root*/ WITH manager_id IS NULL"),
        "hierarchical START WITH clause should remain in the SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn oracle_connect_by_clause_with_inline_comment_is_not_misclassified_as_sqlplus_connect_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT employee_id");
    engine.process_line("FROM employees");
    engine.process_line("START WITH manager_id IS NULL");
    engine.process_line("CONNECT /*hierarchical*/ BY PRIOR employee_id = manager_id;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("CONNECT /*hierarchical*/ BY PRIOR employee_id = manager_id"),
        "hierarchical CONNECT BY clause should remain in the SELECT statement: {}",
        statements[0]
    );
}

#[test]
fn external_language_clause_splits_before_trailing_block_comment_and_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn2 RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("/* next statement comment */");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        !statements[0].contains("next statement comment"),
        "block comment after external routine should belong to next statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("/* next statement comment */\nSELECT 2 FROM dual"),
        "block comment should stay with the following statement: {}",
        statements[1]
    );
}

#[test]
fn non_cte_with_clause_keyword_does_not_leak_into_following_comment_on_function() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("GRANT CREATE SESSION TO app_user WITH ADMIN OPTION;");
    engine.process_line("COMMENT ON FUNCTION app_user.f IS 'ok';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("GRANT CREATE SESSION TO app_user WITH ADMIN OPTION"),
        "first statement should remain the GRANT statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("COMMENT ON FUNCTION app_user.f IS 'ok'"),
        "second statement should remain a standalone COMMENT ON FUNCTION statement: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 1 FROM dual"),
        "third statement should remain a standalone SELECT statement: {}",
        statements[2]
    );
}

#[test]
fn non_cte_with_clause_keyword_does_not_leak_into_following_comment_on_procedure() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("GRANT CREATE SESSION TO app_user WITH ADMIN OPTION;");
    engine.process_line("COMMENT ON PROCEDURE app_user.p IS 'ok';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("GRANT CREATE SESSION TO app_user WITH ADMIN OPTION"),
        "first statement should remain the GRANT statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("COMMENT ON PROCEDURE app_user.p IS 'ok'"),
        "second statement should remain a standalone COMMENT ON PROCEDURE statement: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 1 FROM dual"),
        "third statement should remain a standalone SELECT statement: {}",
        statements[2]
    );
}

#[test]
fn non_cte_with_delegate_option_does_not_leak_into_following_comment_on_function() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("GRANT READ ON DIRECTORY app_dir TO app_user WITH DELEGATE OPTION;");
    engine.process_line("COMMENT ON FUNCTION app_user.f IS 'ok';");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0]
            .starts_with("GRANT READ ON DIRECTORY app_dir TO app_user WITH DELEGATE OPTION"),
        "first statement should remain the GRANT statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("COMMENT ON FUNCTION app_user.f IS 'ok'"),
        "second statement should remain a standalone COMMENT ON FUNCTION statement: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 1 FROM dual"),
        "third statement should remain a standalone SELECT statement: {}",
        statements[2]
    );
}

#[test]
fn non_plsql_grant_with_clause_exits_pending_with_mode_without_semicolon() {
    let mut state = SplitState::default();

    state.track_top_level_with_plsql("GRANT", true);
    state.track_top_level_with_plsql("SELECT", false);
    state.track_top_level_with_plsql("ON", false);
    state.track_top_level_with_plsql("DUAL", false);
    state.track_top_level_with_plsql("TO", false);
    state.track_top_level_with_plsql("APP_USER", false);
    state.track_top_level_with_plsql("WITH", false);
    state.track_top_level_with_plsql("GRANT", false);

    assert_eq!(
        state.with_clause_state,
        WithClauseState::None,
        "WITH GRANT OPTION should immediately exit WITH FUNCTION/PROCEDURE tracking"
    );
}

#[test]
fn non_plsql_grant_with_delegate_option_exits_pending_with_mode_without_semicolon() {
    let mut state = SplitState::default();

    state.track_top_level_with_plsql("GRANT", true);
    state.track_top_level_with_plsql("APP_ROLE", false);
    state.track_top_level_with_plsql("TO", false);
    state.track_top_level_with_plsql("APP_USER", false);
    state.track_top_level_with_plsql("WITH", false);
    state.track_top_level_with_plsql("DELEGATE", false);

    assert_eq!(
        state.with_clause_state,
        WithClauseState::None,
        "WITH DELEGATE OPTION should immediately exit WITH FUNCTION/PROCEDURE tracking"
    );
}

#[test]
fn implicit_external_language_clause_splits_before_following_begin_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_begin RETURN NUMBER");
    engine.process_line("AS LANGUAGE C");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(statements[1].starts_with("BEGIN\n  NULL;\nEND"));
    assert!(statements[2].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn implicit_external_language_clause_on_procedure_splits_before_following_begin_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE ext_proc_begin");
    engine.process_line("AS LANGUAGE C");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 101 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(statements[1].starts_with("BEGIN\n  NULL;\nEND"));
    assert!(statements[2].starts_with("SELECT 101 FROM dual"));
}

#[test]
fn implicit_external_literal_target_clause_on_procedure_with_semicolon_keeps_block_together() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE ext_proc_begin_literal");
    engine.process_line("AS LANGUAGE 'C';");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 102 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE 'C'"));
    assert!(statements[0].contains("BEGIN\n  NULL;\nEND"));
    assert!(statements[1].starts_with("SELECT 102 FROM dual"));
}

#[test]
fn explicit_external_language_clause_splits_before_following_begin_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_begin_explicit RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE C;");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 39 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL LANGUAGE C"));
    assert!(statements[1].starts_with("BEGIN\n  NULL;\nEND"));
    assert!(statements[2].starts_with("SELECT 39 FROM dual"));
}

#[test]
fn implicit_external_literal_target_clause_splits_before_following_begin_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_begin_literal RETURN NUMBER");
    engine.process_line("AS LANGUAGE 'C';");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 40 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE 'C'"));
    assert!(statements[1].starts_with("BEGIN\n  NULL;\nEND"));
    assert!(statements[2].starts_with("SELECT 40 FROM dual"));
}

#[test]
fn external_language_clause_splits_before_run_script_marker_at_sign() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_at RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("@next_script.sql");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("@next_script.sql"),
        "run-script marker should start the next statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_run_script_marker_double_at() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_double_at RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("@@child_script.sql");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("@@child_script.sql"),
        "double run-script marker should start the next statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn with_function_waiting_main_query_recovers_on_slash_line_with_block_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("/ /* rerun statement */");
    engine.process_line("SELECT f() FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with(
            "WITH
  FUNCTION f RETURN NUMBER IS"
        ),
        "WITH FUNCTION declaration should remain the first statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with(
            "/ /* rerun statement */
SELECT f() FROM dual"
        ),
        "slash terminator with block comment should start the next statement: {}",
        statements[1]
    );
}

#[test]
fn with_function_waiting_main_query_recovers_on_slash_line_with_sqlplus_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("/");
    engine.process_line("-- rerun statement");
    engine.process_line("SELECT f() FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "WITH FUNCTION declaration should remain the first statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("/\n-- rerun statement\nSELECT f() FROM dual"),
        "slash terminator with SQL*Plus comment should start the next statement: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_slash_line_with_sqlplus_remark() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_slash_rem RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("/ REM rerun external");
    engine.process_line("SELECT 52 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(
        statements[1].starts_with("SELECT 52 FROM dual"),
        "slash delimiter line should not leak into next statement: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_lowercase_sqlplus_remark_on_slash_line() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_slash_rem_lower RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("/ remark rerun external");
    engine.process_line("SELECT 152 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(
        statements[1].starts_with("SELECT 152 FROM dual"),
        "slash delimiter line should not leak into next statement: {}",
        statements[1]
    );
}

#[test]
fn with_function_waiting_main_query_recovers_on_lowercase_sqlplus_remark_slash_line() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH");
    engine.process_line("  FUNCTION f RETURN NUMBER IS");
    engine.process_line("  BEGIN");
    engine.process_line("    RETURN 1;");
    engine.process_line("  END;");
    engine.process_line("/ rem keep parsing");
    engine.process_line("SELECT f() FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].starts_with("WITH\n  FUNCTION f RETURN NUMBER IS"),
        "WITH FUNCTION declaration should remain the first statement: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("/ rem keep parsing\nSELECT f() FROM dual"),
        "slash line with lowercase rem should start the next statement: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_sqlplus_slash_terminator_line() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_slash RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("/");
    engine.process_line("SELECT 51 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(
        statements[1].starts_with("SELECT 51 FROM dual"),
        "slash delimiter line should not leak into next statement: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_slash_line_with_block_comment() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_slash_block RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("/ /* rerun external */");
    engine.process_line("SELECT 251 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(
        statements[1].starts_with("SELECT 251 FROM dual"),
        "slash line with block comment should be consumed as terminator: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_prompt_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_prompt RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("PROMPT after external");
    engine.process_line("SELECT 33 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert_eq!(
        statements[1],
        "PROMPT after external\nSELECT 33 FROM dual".to_string()
    );
}

#[test]
fn external_language_clause_splits_before_host_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_host RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("HOST ls");
    engine.process_line("SELECT 34 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert_eq!(statements[1], "HOST ls\nSELECT 34 FROM dual".to_string());
}

#[test]
fn external_language_clause_splits_before_bang_host_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_bang_host RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("! ls");
    engine.process_line("SELECT 35 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert_eq!(statements[1], "! ls\nSELECT 35 FROM dual;".to_string());
}

#[test]
fn external_language_clause_splits_before_exit_command() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_exit RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("EXIT");
    engine.process_line("SELECT 36 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert_eq!(statements[1], "EXIT\nSELECT 36 FROM dual".to_string());
}

#[test]
fn external_language_clause_splits_before_create_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_next_create RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("CREATE TABLE t_after_ext (id NUMBER);");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("CREATE TABLE t_after_ext"),
        "CREATE statement should begin a new statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn trigger_referencing_alias_with_when_clause_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW AS n");
    engine.process_line("FOR EACH ROW");
    engine.process_line("WHEN (n.id IS NULL)");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn simple_trigger_call_body_without_as_is_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_call_only");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("FOR EACH ROW");
    engine.process_line("CALL pkg_trg.fire();");
    engine.process_line("SELECT 42 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("CALL pkg_trg.fire()"));
    assert_eq!(statements[1], "SELECT 42 FROM dual".to_string());
}

#[test]
fn package_spec_with_subprogram_declarations_keeps_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE pkg_tmp IS");
    engine.process_line("  FUNCTION f RETURN NUMBER;");
    engine.process_line("  PROCEDURE p;");
    engine.process_line("END pkg_tmp;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("FUNCTION f RETURN NUMBER;"));
    assert!(statements[1].starts_with("SELECT 1 FROM dual"));
}

#[test]
fn with_function_followed_by_lock_statement_recovers() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("LOCK TABLE emp IN EXCLUSIVE MODE;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[1].starts_with("LOCK TABLE emp IN EXCLUSIVE MODE"));
}

#[test]
fn with_function_followed_by_run_script_marker_recovers() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("@child.sql");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(statements[1].starts_with("@child.sql"));
}

#[test]
fn with_function_waiting_main_query_recovers_on_sqlplus_slash_terminator_line() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("/");
    engine.process_line("SELECT 52 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END"));
    assert!(
        statements[1].starts_with("/\nSELECT 52 FROM dual"),
        "slash marker line should start the next statement: {}",
        statements[1]
    );
}

#[test]
fn sqlplus_spool_command_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SPOOL out.log");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec![
            "SPOOL out.log".to_string(),
            "SELECT 1 FROM dual".to_string()
        ]
    );
}

#[test]
fn sqlplus_set_command_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SET SERVEROUTPUT ON");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec![
            "SET SERVEROUTPUT ON".to_string(),
            "SELECT 1 FROM dual".to_string()
        ]
    );
}

#[test]
fn sqlplus_set_command_with_block_comment_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SET /*sqlplus*/ SERVEROUTPUT ON");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec![
            "SET /*sqlplus*/ SERVEROUTPUT ON".to_string(),
            "SELECT 1 FROM dual".to_string()
        ]
    );
}

#[test]
fn sqlplus_show_command_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SHOW USER");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec!["SHOW USER".to_string(), "SELECT 1 FROM dual".to_string()]
    );
}

#[test]
fn sqlplus_describe_command_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("DESC emp");
    engine.process_line("SELECT 53 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec!["DESC emp".to_string(), "SELECT 53 FROM dual".to_string()]
    );
}

#[test]
fn sqlplus_execute_command_is_auto_terminated() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("EXEC dbms_output.put_line('x')");
    engine.process_line("SELECT 54 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(
        statements,
        vec![
            "EXEC dbms_output.put_line('x')".to_string(),
            "SELECT 54 FROM dual".to_string(),
        ]
    );
}

#[test]
fn external_language_clause_splits_before_alter_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_next_alter RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("ALTER SESSION SET optimizer_mode = ALL_ROWS;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("ALTER SESSION SET optimizer_mode = ALL_ROWS"),
        "ALTER statement should begin a new statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn external_language_clause_splits_before_startup_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_next_startup RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("STARTUP;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("STARTUP"),
        "STARTUP command should begin a new statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn sqlplus_startup_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("STARTUP");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "STARTUP".to_string());
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn sqlplus_shutdown_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SHUTDOWN IMMEDIATE");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "SHUTDOWN IMMEDIATE".to_string());
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn sqlplus_archive_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("ARCHIVE LOG LIST");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "ARCHIVE LOG LIST".to_string());
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn sqlplus_recover_command_keeps_following_statement_separate_without_semicolon() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("RECOVER DATABASE");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert_eq!(statements[0], "RECOVER DATABASE".to_string());
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}
#[test]
fn external_language_clause_splits_before_shutdown_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_next_shutdown RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("SHUTDOWN;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SHUTDOWN"),
        "SHUTDOWN command should begin a new statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn procedure_with_implicit_language_target_splits_before_following_statement() {
    for target in ["C", "JAVASCRIPT", "MLE"] {
        let mut engine = SqlParserEngine::new();

        engine.process_line("CREATE OR REPLACE PROCEDURE ext_proc_implicit");
        engine.process_line(&format!("AS LANGUAGE {target};"));
        engine.process_line("SELECT 1 FROM dual;");

        let statements = engine.finalize_and_take_statements();

        assert_eq!(
            statements.len(),
            2,
            "unexpected statements for {target}: {statements:?}"
        );
        assert!(
            statements[0].contains(&format!("AS LANGUAGE {target};")),
            "first statement should keep implicit language target clause for {target}: {}",
            statements[0]
        );
        assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
    }
}

#[test]
fn external_language_clause_splits_before_recover_statement_head_with_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_recover_head RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("RECOVER DATABASE;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep external routine: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("RECOVER DATABASE"),
        "RECOVER should begin a new statement after external routine split: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 1 FROM dual"),
        "SELECT should remain standalone after RECOVER recovery split: {}",
        statements[2]
    );
}

#[test]
fn external_language_clause_splits_before_archive_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_archive_head RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("ARCHIVE LOG LIST;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep external routine: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("ARCHIVE LOG LIST"),
        "ARCHIVE command should begin a new statement after external routine split: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT 1 FROM dual"),
        "SELECT should remain standalone after ARCHIVE recovery split: {}",
        statements[2]
    );
}

#[test]
fn external_language_clause_splits_before_recover_statement_head_without_following_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_fn_next_recover RETURN NUMBER");
    engine.process_line("AS LANGUAGE C;");
    engine.process_line("RECOVER DATABASE;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS LANGUAGE C"),
        "first statement should keep EXTERNAL call spec: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("RECOVER DATABASE"),
        "RECOVER statement should begin a new statement after external routine split: {}",
        statements[1]
    );
}

#[test]
fn with_function_recovers_before_alter_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD';");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'"),
        "ALTER statement should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after ALTER recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_before_create_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("CREATE TABLE t_recovery_head (id NUMBER);");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("CREATE TABLE t_recovery_head (id NUMBER)"),
        "CREATE statement should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after CREATE recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_before_startup_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("STARTUP;");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("STARTUP"),
        "STARTUP command should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after STARTUP recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_before_shutdown_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("SHUTDOWN;");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("SHUTDOWN"),
        "SHUTDOWN command should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after SHUTDOWN recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_before_administer_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("ADMINISTER KEY MANAGEMENT SET KEY IDENTIFIED BY \"pwd\";");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("ADMINISTER KEY MANAGEMENT"),
        "ADMINISTER statement should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after ADMINISTER recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_recovers_before_recover_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION local_fn RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END local_fn;");
    engine.process_line("RECOVER DATABASE;");
    engine.process_line("SELECT local_fn() FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 3, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END local_fn"),
        "first statement should keep WITH FUNCTION declaration: {}",
        statements[0]
    );
    assert!(
        statements[1].starts_with("RECOVER DATABASE"),
        "RECOVER statement should start a new statement after WITH FUNCTION recovery: {}",
        statements[1]
    );
    assert!(
        statements[2].starts_with("SELECT local_fn() FROM dual"),
        "SELECT statement should remain standalone after RECOVER recovery split: {}",
        statements[2]
    );
}

#[test]
fn with_function_followed_by_parenthesized_main_query_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH FUNCTION f RETURN NUMBER IS");
    engine.process_line("BEGIN");
    engine.process_line("  RETURN 1;");
    engine.process_line("END;");
    engine.process_line("(SELECT f() AS v FROM dual)");
    engine.process_line("UNION ALL");
    engine.process_line("SELECT 2 AS v FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("(SELECT f() AS v FROM dual)"),
        "parenthesized main query should remain attached to WITH FUNCTION statement: {}",
        statements[0]
    );
    assert!(
        statements[0].ends_with("SELECT 2 AS v FROM dual"),
        "union tail should remain attached: {}",
        statements[0]
    );
}

#[test]
fn with_procedure_followed_by_parenthesized_main_query_stays_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("WITH PROCEDURE p IS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("(SELECT 1 AS v FROM dual)");
    engine.process_line("UNION ALL");
    engine.process_line("SELECT 2 AS v FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 1, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("(SELECT 1 AS v FROM dual)"),
        "parenthesized main query should remain attached to WITH PROCEDURE statement: {}",
        statements[0]
    );
    assert!(
        statements[0].ends_with("SELECT 2 AS v FROM dual"),
        "union tail should remain attached: {}",
        statements[0]
    );
}

#[test]
fn trigger_follows_precedes_and_instead_of_forms_split_normally() {
    let cases = [
        (
            vec![
                "CREATE OR REPLACE TRIGGER trg_follows",
                "AFTER INSERT ON emp",
                "FOLLOWS trg_base",
                "BEGIN",
                "  NULL;",
                "END;",
                "SELECT 1 FROM dual;",
            ],
            "SELECT 1 FROM dual",
        ),
        (
            vec![
                "CREATE OR REPLACE TRIGGER trg_precedes",
                "BEFORE UPDATE ON emp",
                "PRECEDES trg_base",
                "BEGIN",
                "  NULL;",
                "END;",
                "SELECT 2 FROM dual;",
            ],
            "SELECT 2 FROM dual",
        ),
        (
            vec![
                "CREATE OR REPLACE TRIGGER trg_instead_view",
                "INSTEAD OF INSERT ON emp_v",
                "BEGIN",
                "  NULL;",
                "END;",
                "SELECT 3 FROM dual;",
            ],
            "SELECT 3 FROM dual",
        ),
    ];

    for (lines, tail_head) in cases {
        let mut engine = SqlParserEngine::new();
        for line in lines {
            engine.process_line(line);
        }

        let statements = engine.finalize_and_take_statements();
        assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
        assert!(
            statements[1].starts_with(tail_head),
            "trailing SELECT should split from trigger DDL: {}",
            statements[1]
        );
    }
}

#[test]
fn trigger_referencing_alias_with_quoted_identifier_keeps_call_body_is_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias_quoted_call_is");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW IS \"N\"");
    engine.process_line("FOR EACH ROW");
    engine.process_line("IS");
    engine.process_line("CALL pkg_trg.fire();");
    engine.process_line("SELECT 37 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("CALL pkg_trg.fire()"),
        "trigger CALL body should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 37 FROM dual".to_string());
}

#[test]
fn trigger_referencing_alias_with_quoted_identifier_keeps_call_body_as_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TRIGGER trg_ref_alias_quoted_call_as");
    engine.process_line("BEFORE INSERT ON t");
    engine.process_line("REFERENCING NEW AS \"N\"");
    engine.process_line("FOR EACH ROW");
    engine.process_line("AS");
    engine.process_line("CALL pkg_trg.fire();");
    engine.process_line("SELECT 38 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("CALL pkg_trg.fire()"),
        "trigger CALL body should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 38 FROM dual".to_string());
}

#[test]
fn create_function_aggregate_using_clause_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION median_agg(x NUMBER)");
    engine.process_line("RETURN NUMBER");
    engine.process_line("AGGREGATE USING median_impl;");
    engine.process_line("SELECT 39 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AGGREGATE USING median_impl"),
        "AGGREGATE USING call spec should stay in CREATE statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 39 FROM dual".to_string());
}

#[test]
fn create_function_pipelined_using_clause_without_semicolon_uses_slash_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION stream_rows");
    engine.process_line("RETURN row_tab_t PIPELINED");
    engine.process_line("USING stream_rows_impl");
    engine.process_line("/");
    engine.process_line("SELECT 40 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("USING stream_rows_impl"),
        "PIPELINED USING clause should stay in CREATE statement: {}",
        statements[0]
    );
    assert!(
        statements[1].contains("SELECT 40 FROM dual"),
        "trailing SELECT should split after slash terminator: {}",
        statements[1]
    );
}

#[test]
fn package_body_polymorphic_pipelined_using_clause_closes_nested_routine() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_poly AS");
    engine.process_line("  FUNCTION stream_rows RETURN row_tab_t");
    engine.process_line("  IS PIPELINED ROW POLYMORPHIC USING stream_rows_impl;");
    engine.process_line("END pkg_poly;");
    engine.process_line("SELECT 41 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PIPELINED ROW POLYMORPHIC USING stream_rows_impl"),
        "polymorphic PIPELINED USING call spec should remain in package body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 41 FROM dual".to_string());
}

#[test]
fn package_body_table_polymorphic_pipelined_using_clause_closes_nested_routine() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_poly_table AS");
    engine.process_line("  FUNCTION stream_rows RETURN row_tab_t");
    engine.process_line("  IS PIPELINED TABLE POLYMORPHIC USING stream_rows_impl;");
    engine.process_line("END pkg_poly_table;");
    engine.process_line("SELECT 42 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PIPELINED TABLE POLYMORPHIC USING stream_rows_impl"),
        "table polymorphic PIPELINED USING call spec should remain in package body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 42 FROM dual".to_string());
}

#[test]
fn conditional_compilation_directives_do_not_break_following_statement_split() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  $IF $$PLSQL_DEBUG $THEN");
    engine.process_line("    NULL;");
    engine.process_line("  $ELSE");
    engine.process_line("    NULL;");
    engine.process_line("  $END");
    engine.process_line("END;");
    engine.process_line("SELECT 41 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("$IF $$PLSQL_DEBUG $THEN"));
    assert_eq!(statements[1], "SELECT 41 FROM dual".to_string());
}

#[test]
fn language_javascript_mle_module_clause_splits_following_select() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_lang_mle RETURN NUMBER");
    engine.process_line("AS LANGUAGE JAVASCRIPT MLE MODULE ext_mod SIGNATURE 'sig';");
    engine.process_line("SELECT 42 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE JAVASCRIPT MLE MODULE ext_mod"));
    assert_eq!(statements[1], "SELECT 42 FROM dual".to_string());
}

#[test]
fn nested_external_function_in_package_body_keeps_package_statement_intact() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_ext AS");
    engine.process_line("  FUNCTION f RETURN NUMBER");
    engine.process_line("  AS LANGUAGE C NAME 'f';");
    engine.process_line("END pkg_ext;");
    engine.process_line("SELECT 43 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("CREATE OR REPLACE PACKAGE BODY pkg_ext AS"));
    assert!(statements[0].contains("AS LANGUAGE C NAME 'f';"));
    assert!(statements[0].contains("END pkg_ext"));
    assert_eq!(statements[1], "SELECT 43 FROM dual".to_string());
}

#[test]
fn nested_external_function_with_quoted_language_target_closes_subprogram_block() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_ext_quote AS");
    engine.process_line("  FUNCTION f RETURN NUMBER");
    engine.process_line("  AS LANGUAGE 'C' NAME 'f';");
    engine.process_line("END pkg_ext_quote;");
    engine.process_line("SELECT 44 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE 'C' NAME 'f';"));
    assert!(statements[0].contains("END pkg_ext_quote"));
    assert_eq!(statements[1], "SELECT 44 FROM dual".to_string());
}

#[test]
fn quoted_package_body_name_with_initializer_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"Pkg$Ext\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"Pkg$Ext\";");
    engine.process_line("SELECT 55 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"Pkg$Ext\""),
        "quoted package body terminator should stay in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 55 FROM dual".to_string());
}

#[test]
fn type_body_member_external_call_spec_does_not_split_before_type_end() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE TYPE BODY t_ext_member AS");
    engine.process_line("  MAP MEMBER FUNCTION f RETURN NUMBER");
    engine.process_line("  IS LANGUAGE C NAME 'f';");
    engine.process_line("END;");
    engine.process_line("SELECT 45 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("CREATE OR REPLACE TYPE BODY t_ext_member AS"));
    assert!(statements[0].contains("MAP MEMBER FUNCTION f RETURN NUMBER"));
    assert!(statements[0].contains("IS LANGUAGE C NAME 'f';"));
    assert!(statements[0].contains("END"));
    assert_eq!(statements[1], "SELECT 45 FROM dual".to_string());
}

#[test]
fn external_language_target_without_semicolon_splits_before_following_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_plain_lang RETURN NUMBER");
    engine.process_line("AS LANGUAGE C");
    engine.process_line("SELECT 46 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE C"));
    assert!(
        statements[1].starts_with("SELECT 46 FROM dual"),
        "SELECT should begin a new statement after implicit external call spec: {}",
        statements[1]
    );
}

#[test]
fn quoted_package_body_name_with_quoted_end_label_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"Pkg.Ext\" AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END run_me;");
    engine.process_line("END \"Pkg.Ext\";");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"Pkg.Ext\""),
        "quoted package END label should remain attached to package body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn schema_qualified_quoted_package_body_name_with_dot_keeps_end_label_attached() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"App\".\"Pkg.Ext\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"App\".\"Pkg.Ext\";");
    engine.process_line("SELECT 2 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END \"App\".\"Pkg.Ext\""),
        "schema-qualified quoted package END label should remain attached: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 2 FROM dual".to_string());
}

#[test]
fn schema_qualified_quoted_package_body_name_with_split_end_label_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"App\".\"Pkg.Ext\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END");
    engine.process_line("\"App\".\"Pkg.Ext\";");
    engine.process_line("SELECT 3 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END\n\"App\".\"Pkg.Ext\""),
        "split schema-qualified quoted package END label should remain in package body: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 3 FROM dual".to_string());
}

#[test]
fn package_body_nested_routine_named_end_updates_depth_after_end_label() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_depth_chk AS");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("  PROCEDURE run_me IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("    IF 1 = 1 THEN");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("      NULL;");
    engine.process_line("    END IF;");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  END run_me;");
    assert_eq!(
        engine.block_depth(),
        1,
        "END <name> should close nested routine depth"
    );
}

#[test]
fn package_body_inner_begin_end_keeps_nested_member_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_inner_begin AS");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("  PROCEDURE run_me IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("    BEGIN");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("      NULL;");
    engine.process_line("    EXCEPTION");
    engine.process_line("      WHEN OTHERS THEN");
    engine.process_line("        NULL;");
    engine.process_line("    END;");
    assert_eq!(
        engine.block_depth(),
        2,
        "inner BEGIN ... END must not close the enclosing package member"
    );
    engine.process_line("  END run_me;");
    assert_eq!(engine.block_depth(), 1);
}

#[test]
fn package_body_member_after_inner_begin_end_stays_in_same_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_inner_begin AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    BEGIN");
    engine.process_line("      NULL;");
    engine.process_line("    EXCEPTION");
    engine.process_line("      WHEN OTHERS THEN");
    engine.process_line("        NULL;");
    engine.process_line("    END;");
    engine.process_line("  END run_me;");
    engine.process_line("  PROCEDURE run_next IS");
    engine.process_line("    v_cnt NUMBER := 0;");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END run_next;");
    engine.process_line("END pkg_inner_begin;");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE run_me IS"),
        "first package member should remain in package body: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("PROCEDURE run_next IS\n    v_cnt NUMBER := 0;\n  BEGIN"),
        "following package member should not split away after inner BEGIN ... END: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END pkg_inner_begin"),
        "package body terminator should stay with the package statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn package_body_local_nested_subprograms_keep_member_and_initializer_depths() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY fmt_nested_pkg AS");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("  PROCEDURE run_demo (p_seed IN NUMBER DEFAULT 3, p_result OUT CLOB) IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line(
        "    PROCEDURE process_row (p_row IN t_row, p_depth IN PLS_INTEGER DEFAULT 1) IS",
    );
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("      PROCEDURE nested_walk (p_start IN PLS_INTEGER) IS");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("      BEGIN");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("        NULL;");
    engine.process_line("      END nested_walk;");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("    BEGIN");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("      FOR j IN REVERSE 1 .. 2 LOOP");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("        BEGIN");
    assert_eq!(engine.block_depth(), 5);
    engine.process_line("          NULL;");
    engine.process_line("        EXCEPTION");
    engine.process_line("          WHEN OTHERS THEN");
    engine.process_line("            NULL;");
    engine.process_line("        END;");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("      END LOOP;");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("    END process_row;");
    assert_eq!(
        engine.block_depth(),
        2,
        "local nested procedure END should restore enclosing package member depth"
    );
    engine.process_line("  BEGIN");
    assert_eq!(
        engine.block_depth(),
        2,
        "package member body BEGIN should not stay nested under the local procedure"
    );
    engine.process_line("    NULL;");
    engine.process_line("  END run_demo;");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("BEGIN");
    assert_eq!(
        engine.block_depth(),
        2,
        "package body initializer BEGIN should start after nested member closes"
    );
    engine.process_line("  NULL;");
    engine.process_line("END fmt_nested_pkg;");
    assert_eq!(engine.block_depth(), 0);
}

#[test]
fn anonymous_declare_keeps_pending_begin_after_local_subprogram_body() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("DECLARE");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("  PROCEDURE bump IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  BEGIN");
    assert_eq!(
        engine.block_depth(),
        2,
        "local subprogram body BEGIN should not consume the outer DECLARE ... BEGIN depth"
    );
    engine.process_line("    NULL;");
    engine.process_line("  END;");
    assert_eq!(engine.block_depth(), 1);
    assert_eq!(
        engine.state.pending_subprogram_begins, 0,
        "local subprogram END should clear pending subprogram begin tracking"
    );
    engine.process_line("BEGIN");
    assert_eq!(
        engine.block_depth(),
        1,
        "outer anonymous block BEGIN should remain at DECLARE depth after local subprogram"
    );
    engine.process_line("  NULL;");
    engine.process_line("END;");
    assert_eq!(engine.block_depth(), 0);
}

#[test]
fn package_body_torture_blocks_remain_single_statement_until_terminator() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY torture_pkg");
    engine.process_line("IS");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("FUNCTION log_message(p_msg VARCHAR2)");
    engine.process_line("RETURN NUMBER");
    engine.process_line("IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("PRAGMA AUTONOMOUS_TRANSACTION;");
    engine.process_line("BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  RETURN 1;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    RETURN -1;");
    engine.process_line("END;");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("PROCEDURE bulk_raise_salary");
    engine.process_line("IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("  FORALL i IN 1 .. v_ids.COUNT SAVE EXCEPTIONS");
    engine.process_line("      UPDATE emp");
    engine.process_line("      SET sal = sal * 1.1");
    engine.process_line("      WHERE empno = v_ids(i);");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    FOR i IN 1 .. SQL%BULK_EXCEPTIONS.COUNT LOOP");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("      NULL;");
    engine.process_line("    END LOOP;");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("END;");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("END torture_pkg;");
    assert_eq!(engine.block_depth(), 0);
    engine.process_line("/");
    engine.process_line("SELECT 1 FROM dual;");

    let statements = engine.finalize_and_take_statements();

    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("PROCEDURE bulk_raise_salary"),
        "bulk_raise_salary should remain inside the package body statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("SQL%BULK_EXCEPTIONS.COUNT"),
        "cursor attribute references should remain attached inside the package statement: {}",
        statements[0]
    );
    assert!(
        statements[0].contains("END torture_pkg"),
        "package body terminator should stay in the same statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 1 FROM dual".to_string());
}

#[test]
fn package_body_consecutive_plain_case_ends_restore_following_member_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY oqt_mega_pkg AS");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("  FUNCTION f_deep RETURN NUMBER IS");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("    v NUMBER := 0;");
    engine.process_line("  BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("    v :=");
    engine.process_line("      CASE");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("        WHEN 1 = 1 THEN");
    engine.process_line("          CASE");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("            WHEN 2 = 2 THEN 100");
    engine.process_line("            ELSE 10");
    engine.process_line("          END");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("        ELSE");
    assert_eq!(engine.block_depth(), 3);
    engine.process_line("          CASE");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("            WHEN 3 = 3 THEN 777");
    engine.process_line("            ELSE 0");
    engine.process_line("          END");
    assert_eq!(engine.block_depth(), 4);
    engine.process_line("      END;");
    assert_eq!(
        engine.block_depth(),
        2,
        "outer CASE END; should fully unwind nested CASE expressions before the routine END"
    );
    engine.process_line("    RETURN v;");
    engine.process_line("  END;");
    assert_eq!(
        engine.block_depth(),
        1,
        "function END after consecutive CASE END tokens should restore package-body depth"
    );
    engine.process_line("  PROCEDURE run_torture IS");
    assert_eq!(
        engine.block_depth(),
        2,
        "following package member should not stay nested under the previous function"
    );
    engine.process_line("  BEGIN");
    assert_eq!(engine.block_depth(), 2);
    engine.process_line("    NULL;");
    engine.process_line("  END run_torture;");
    assert_eq!(engine.block_depth(), 1);
    engine.process_line("END oqt_mega_pkg;");
    assert_eq!(engine.block_depth(), 0);
}

#[test]
fn slash_terminator_with_block_comment_then_line_comment_is_consumed() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");
    engine.process_line("/ /* keep */ -- slash terminator comment");
    engine.process_line("SELECT 47 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].starts_with("BEGIN"));
    assert_eq!(statements[1], "SELECT 47 FROM dual".to_string());
}

#[test]
fn oracle_external_name_identifier_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_name_ident RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE C NAME ext_symbol;");
    engine.process_line("SELECT 48 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL LANGUAGE C NAME ext_symbol"),
        "external call spec should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 48 FROM dual".to_string());
}

#[test]
fn oracle_external_name_quoted_identifier_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_name_qident RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE C NAME \"Ext$Sym\";");
    engine.process_line("SELECT 49 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL LANGUAGE C NAME \"Ext$Sym\""),
        "quoted identifier target should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 49 FROM dual".to_string());
}

#[test]
fn oracle_external_clause_accepts_language_after_name_identifier() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_name_then_language RETURN NUMBER");
    engine.process_line("AS EXTERNAL NAME ext_symbol LANGUAGE C;");
    engine.process_line("SELECT 50 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL NAME ext_symbol LANGUAGE C"),
        "post-NAME LANGUAGE target should remain in call spec: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 50 FROM dual".to_string());
}

#[test]
fn oracle_external_clause_accepts_language_after_name_quoted_identifier() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_qname_then_language RETURN NUMBER");
    engine.process_line("AS EXTERNAL NAME \"Ext$Sym\" LANGUAGE C;");
    engine.process_line("SELECT 51 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("AS EXTERNAL NAME \"Ext$Sym\" LANGUAGE C"),
        "quoted NAME target with trailing LANGUAGE should remain in call spec: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 51 FROM dual".to_string());
}

#[test]
fn package_body_initializer_with_nested_if_and_exception_keeps_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_depth AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  ELSE");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END pkg_depth;");
    engine.process_line("SELECT 100 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END IF;"));
    assert!(statements[0].contains("EXCEPTION"));
    assert!(statements[0].contains("END pkg_depth"));
    assert_eq!(statements[1], "SELECT 100 FROM dual".to_string());
}

#[test]
fn package_body_nested_routine_end_name_with_if_else_exception_keeps_package_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    IF 1 = 1 THEN");
    engine.process_line("      NULL;");
    engine.process_line("    ELSE");
    engine.process_line("      NULL;");
    engine.process_line("    END IF;");
    engine.process_line("  EXCEPTION");
    engine.process_line("    WHEN OTHERS THEN");
    engine.process_line("      NULL;");
    engine.process_line("  END run_me;");
    engine.process_line("END pkg_nested;");
    engine.process_line("SELECT 101 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END IF;"));
    assert!(statements[0].contains("END run_me"));
    assert!(
        statements[0].contains("END pkg_nested"),
        "package body end label moved out of first statement: {statements:?}"
    );
    assert_eq!(statements[1], "SELECT 101 FROM dual".to_string());
}

#[test]
fn package_body_end_with_qualified_label_still_closes_outer_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_label_qualified AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  ELSE");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END owner.pkg_label_qualified;");
    engine.process_line("SELECT 103 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END owner.pkg_label_qualified"),
        "qualified package end label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 103 FROM dual".to_string());
}

#[test]
fn package_body_end_with_three_part_qualified_label_closes_outer_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_label_tripart AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  ELSE");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END db.owner.pkg_label_tripart;");
    engine.process_line("SELECT 104 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("END db.owner.pkg_label_tripart"),
        "three-part qualified package end label should remain in first statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 104 FROM dual".to_string());
}

#[test]
fn package_body_nested_routine_end_with_qualified_name_keeps_single_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY pkg_nested_q AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    IF 1 = 1 THEN");
    engine.process_line("      NULL;");
    engine.process_line("    ELSE");
    engine.process_line("      NULL;");
    engine.process_line("    END IF;");
    engine.process_line("  EXCEPTION");
    engine.process_line("    WHEN OTHERS THEN");
    engine.process_line("      NULL;");
    engine.process_line("  END owner.run_me;");
    engine.process_line("END pkg_nested_q;");
    engine.process_line("SELECT 105 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END owner.run_me"));
    assert!(statements[0].contains("END pkg_nested_q"));
    assert_eq!(statements[1], "SELECT 105 FROM dual".to_string());
}

#[test]
fn oracle_external_language_without_semicolon_splits_before_following_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_external_no_semi RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE C");
    engine.process_line("SELECT 50 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL LANGUAGE C"));
    assert_eq!(statements[1], "SELECT 50 FROM dual".to_string());
}

#[test]
fn create_function_is_language_call_spec_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_is_lang RETURN NUMBER");
    engine.process_line("IS LANGUAGE C NAME 'ext_is_lang'");
    engine.process_line("SELECT 777 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("IS LANGUAGE C NAME 'ext_is_lang'"),
        "external call-spec must stay in CREATE FUNCTION statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 777 FROM dual".to_string());
}

#[test]
fn create_procedure_is_language_java_name_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PROCEDURE ext_proc_is_lang");
    engine.process_line("IS LANGUAGE JAVA NAME 'pkg.Ext.proc()'");
    engine.process_line("SELECT 778 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("IS LANGUAGE JAVA NAME 'pkg.Ext.proc()'"),
        "external Java call-spec must stay in CREATE PROCEDURE statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 778 FROM dual".to_string());
}

#[test]
fn malformed_external_clause_without_language_target_still_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_missing_lang_target RETURN NUMBER");
    engine.process_line("AS EXTERNAL;");
    engine.process_line("SELECT 51 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL"));
    assert_eq!(statements[1], "SELECT 51 FROM dual".to_string());
}

#[test]
fn malformed_implicit_language_clause_keyword_without_target_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_missing_implicit_target RETURN NUMBER");
    engine.process_line("AS LANGUAGE PARAMETERS ('x')");
    engine.process_line("SELECT 52 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE PARAMETERS ('x')"));
    assert_eq!(statements[1], "SELECT 52 FROM dual".to_string());
}

#[test]
fn malformed_mle_clause_without_module_or_signature_still_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_missing_target RETURN NUMBER");
    engine.process_line("AS MLE;");
    engine.process_line("SELECT 53 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS MLE"));
    assert_eq!(statements[1], "SELECT 53 FROM dual".to_string());
}

#[test]
fn malformed_using_clause_without_target_still_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_using_missing_target RETURN NUMBER");
    engine.process_line("AS AGGREGATE USING;");
    engine.process_line("SELECT 54 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS AGGREGATE USING"));
    assert_eq!(statements[1], "SELECT 54 FROM dual".to_string());
}

#[test]
fn malformed_external_clause_without_semicolon_splits_before_following_statement_head() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_missing_external_semicolon RETURN NUMBER");
    engine.process_line("AS EXTERNAL");
    engine.process_line("SELECT 57 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL"));
    assert_eq!(statements[1], "SELECT 57 FROM dual".to_string());
}

#[test]
fn malformed_implicit_language_with_quoted_target_without_semicolon_splits_before_following_statement_head(
) {
    let mut engine = SqlParserEngine::new();

    engine.process_line(
        "CREATE OR REPLACE FUNCTION ext_missing_quoted_implicit_semicolon RETURN NUMBER",
    );
    engine.process_line("AS LANGUAGE \"C\"");
    engine.process_line("SELECT 59 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE \"C\""));
    assert_eq!(statements[1], "SELECT 59 FROM dual".to_string());
}

#[test]
fn oracle_mle_module_clause_after_quoted_language_target_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_module RETURN NUMBER");
    engine.process_line("AS LANGUAGE \"JavaScript\" MODULE ext_mod;");
    engine.process_line("SELECT 58 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE \"JavaScript\" MODULE ext_mod"),
        "MODULE clause should remain in external call-spec statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 58 FROM dual".to_string());
}

#[test]
fn oracle_mle_signature_clause_after_quoted_language_target_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_sig RETURN NUMBER");
    engine.process_line("AS LANGUAGE \"JavaScript\" SIGNATURE 'f()';");
    engine.process_line("SELECT 59 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("LANGUAGE \"JavaScript\" SIGNATURE 'f()'"),
        "SIGNATURE clause should remain in external call-spec statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 59 FROM dual".to_string());
}

#[test]
fn malformed_implicit_language_with_quoted_target_without_semicolon_splits_before_following_begin_block(
) {
    let mut engine = SqlParserEngine::new();

    engine
        .process_line("CREATE OR REPLACE FUNCTION ext_missing_quoted_implicit_begin RETURN NUMBER");
    engine.process_line("AS LANGUAGE \"C\"");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS LANGUAGE \"C\""));
    assert_eq!(statements[1], "BEGIN\n  NULL;\nEND".to_string());
}

#[test]
fn malformed_external_language_without_target_or_semicolon_splits_before_following_statement_head()
{
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_missing_language_target RETURN NUMBER");
    engine.process_line("AS EXTERNAL LANGUAGE");
    engine.process_line("SELECT 58 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("AS EXTERNAL LANGUAGE"));
    assert_eq!(statements[1], "SELECT 58 FROM dual".to_string());
}

#[test]
fn package_body_named_if_handles_nested_if_and_init_end_if_correctly() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY if AS");
    engine.process_line("BEGIN");
    engine.process_line("  IF 1 = 1 THEN");
    engine.process_line("    NULL;");
    engine.process_line("  ELSE");
    engine.process_line("    NULL;");
    engine.process_line("  END IF;");
    engine.process_line("END IF;");
    engine.process_line("SELECT 55 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END IF;\nEND IF"));
    assert_eq!(statements[1], "SELECT 55 FROM dual".to_string());
}

#[test]
fn package_body_init_exception_block_with_keyword_label_keeps_depth_balanced() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY exception AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("EXCEPTION");
    engine.process_line("  WHEN OTHERS THEN");
    engine.process_line("    NULL;");
    engine.process_line("END exception;");
    engine.process_line("SELECT 56 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("EXCEPTION\n  WHEN OTHERS THEN"));
    assert_eq!(statements[1], "SELECT 56 FROM dual".to_string());
}

#[test]
fn oracle_mle_env_import_export_clause_after_quoted_language_target_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_mle_env RETURN NUMBER");
    engine.process_line(
        "AS LANGUAGE \"JavaScript\" ENV env_ctx IMPORT imports_mod EXPORT exports_mod;",
    );
    engine.process_line("SELECT 78 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0]
            .contains("LANGUAGE \"JavaScript\" ENV env_ctx IMPORT imports_mod EXPORT exports_mod"),
        "ENV/IMPORT/EXPORT clauses should remain in external call-spec statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 78 FROM dual".to_string());
}

#[test]
fn package_body_name_end_if_closes_outer_as_is_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY if AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END run_me;");
    engine.process_line("END IF;");
    assert_eq!(
        engine.block_depth(),
        0,
        "END IF label for package body should close outer AS/IS depth"
    );
}

#[test]
fn quoted_package_body_name_end_label_splits_before_next_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"Pkg$Quoted\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"Pkg$Quoted\";");
    engine.process_line("SELECT 77 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END \"Pkg$Quoted\""));
    assert_eq!(statements[1], "SELECT 77 FROM dual".to_string());
}

#[test]
fn package_body_name_end_exception_closes_outer_as_is_depth() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY exception AS");
    engine.process_line("  PROCEDURE run_me IS");
    engine.process_line("  BEGIN");
    engine.process_line("    NULL;");
    engine.process_line("  END run_me;");
    engine.process_line("END EXCEPTION;");
    assert_eq!(
        engine.block_depth(),
        0,
        "END EXCEPTION label for package body should close outer AS/IS depth"
    );
}

#[test]
fn select_alias_named_if_does_not_trigger_plsql_if_state_lowercase() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("select 1 as if, 2 as end from dual if;");

    let statements = engine.take_statements();
    assert_eq!(
        statements,
        vec!["select 1 as if, 2 as end from dual if".to_string()]
    );
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.block_depth(), 0);
}

#[test]
fn select_alias_named_if_does_not_trigger_plsql_if_state() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT 1 AS IF, 2 AS END FROM dual IF;");

    let statements = engine.take_statements();
    assert_eq!(
        statements,
        vec!["SELECT 1 AS IF, 2 AS END FROM dual IF".to_string()]
    );
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.block_depth(), 0);
}

#[test]
fn select_alias_named_if_before_case_then_does_not_trigger_plsql_if_state() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("SELECT amount IF, CASE WHEN flag = 1 THEN 1 ELSE 0 END score FROM sales;");

    let statements = engine.take_statements();
    assert_eq!(
        statements,
        vec!["SELECT amount IF, CASE WHEN flag = 1 THEN 1 ELSE 0 END score FROM sales".to_string()]
    );
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.block_depth(), 0);
}

#[test]
fn plsql_select_alias_named_if_does_not_trigger_plsql_if_state() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  SELECT amount AS IF INTO v_amount FROM sales;");
    engine.process_line("END;");

    let statements = engine.take_statements();
    assert_eq!(statements.len(), 1);
    assert!(
        statements[0].contains("SELECT amount AS IF INTO v_amount FROM sales"),
        "statement should preserve SELECT alias IF in PL/SQL block: {}",
        statements[0]
    );
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.block_depth(), 0);
}

#[test]
fn plsql_select_implicit_alias_named_if_does_not_trigger_plsql_if_state() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("BEGIN");
    engine.process_line("  SELECT amount IF INTO v_amount FROM sales;");
    engine.process_line("END;");

    let statements = engine.take_statements();
    assert_eq!(statements.len(), 1);
    assert!(
        statements[0].contains("SELECT amount IF INTO v_amount FROM sales"),
        "statement should preserve implicit SELECT alias IF in PL/SQL block: {}",
        statements[0]
    );
    assert_eq!(engine.state.if_state, IfState::None);
    assert_eq!(engine.state.block_depth(), 0);
}

#[test]
fn package_body_end_with_mismatched_schema_qualified_label_does_not_consume_outer_as_is() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY owner.pkg_match AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END wrong.pkg_match;");
    engine.process_line("SELECT 901 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END wrong.pkg_match"));
    assert_eq!(statements[1], "SELECT 901 FROM dual".to_string());
}

#[test]
fn package_body_end_with_mismatched_quoted_schema_qualified_label_does_not_consume_outer_as_is() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY \"OWNER\".\"PKG_MATCH\" AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END \"WRONG\".\"PKG_MATCH\";");
    engine.process_line("SELECT 902 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END \"WRONG\".\"PKG_MATCH\""));
    assert_eq!(statements[1], "SELECT 902 FROM dual".to_string());
}
#[test]
fn package_body_end_label_with_whitespace_around_dot_splits_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE PACKAGE BODY owner.pkg_ws AS");
    engine.process_line("BEGIN");
    engine.process_line("  NULL;");
    engine.process_line("END owner . pkg_ws;");
    engine.process_line("SELECT 900 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(statements[0].contains("END owner . pkg_ws"));
    assert_eq!(statements[1], "SELECT 900 FROM dual".to_string());
}

#[test]
fn create_function_is_language_rust_call_spec_splits_before_following_statement() {
    let mut engine = SqlParserEngine::new();

    engine.process_line("CREATE OR REPLACE FUNCTION ext_is_lang_rust RETURN NUMBER");
    engine.process_line("IS LANGUAGE RUST NAME 'ext_is_lang_rust'");
    engine.process_line("SELECT 779 FROM dual;");

    let statements = engine.finalize_and_take_statements();
    assert_eq!(statements.len(), 2, "unexpected statements: {statements:?}");
    assert!(
        statements[0].contains("IS LANGUAGE RUST NAME 'ext_is_lang_rust'"),
        "RUST external call-spec must stay in CREATE FUNCTION statement: {}",
        statements[0]
    );
    assert_eq!(statements[1], "SELECT 779 FROM dual".to_string());
}

#[test]
fn slash_on_own_line_inside_parens_is_not_terminator() {
    let input = "SELECT\n    (\n        (1 + 2)\n        /\n        NULLIF(x, 0)\n    ) AS result\nFROM dual";
    let mut engine = SqlParserEngine::new();
    let lines: Vec<&str> = input.lines().collect();

    // Process lines up to the `/` line and check state
    for (i, line) in lines.iter().enumerate() {
        let stmts = engine.process_line_and_take_statements(line);
        let idle = engine.is_idle();
        let pd = engine.paren_depth();
        let bd = engine.block_depth();
        eprintln!(
            "After line {}: {:?} -> idle={}, paren_depth={}, block_depth={}, stmts={:?}",
            i, line, idle, pd, bd, stmts
        );
        if line.trim() == "/" {
            assert!(
                !idle || pd > 0,
                "Parser should not be idle with paren_depth=0 before `/` line"
            );
        }
    }

    let final_stmts = engine.finalize_and_take_statements();
    let all_text: String = final_stmts.join(" | ");
    assert!(
        all_text.contains("/ NULLIF") || all_text.contains("/\n") || all_text.contains("NULLIF"),
        "Division operator should be part of the statement, not treated as terminator: {:?}",
        final_stmts
    );
}
