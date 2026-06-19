use crate::db::{QueryExecutor, SessionState, ToolCommand};

const INTELLISENSE_TEXT_BIND_SCAN_WINDOW: usize = 256 * 1024;

#[derive(Clone)]
struct ExpandedStatementWindow {
    statement_start: usize,
    statement_end: usize,
    text: String,
    cursor_in_statement: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalBlockKind {
    Routine,
    Declare,
    Loop,
    Begin,
    If,
    Case,
}

#[derive(Clone, Copy)]
struct LocalBlockFrame {
    kind: LocalBlockKind,
    scope_id: Option<usize>,
    awaiting_body_begin: bool,
}

#[derive(Clone)]
struct LocalScopeBuilder {
    scope: LocalScope,
    token_start_idx: usize,
    token_end_idx: usize,
    decl_start_idx: Option<usize>,
    decl_end_idx: Option<usize>,
    mysql_declare_statements: bool,
}

#[derive(Clone)]
struct ParsedRoutineHeader {
    body_keyword_idx: usize,
    decl_start_idx: usize,
    parameters: Vec<ParsedDeclarationSymbol>,
    return_type_display: Option<String>,
    body_starts_immediately: bool,
}

#[derive(Clone)]
struct ParsedPackageBodyHeader {
    body_keyword_idx: usize,
    decl_start_idx: usize,
}

struct ParsedForLoopRecord {
    name: String,
    members: Vec<String>,
    member_source_upper: Option<String>,
    member_source_uppers: Vec<String>,
    member_source_is_rowtype: bool,
}

#[derive(Clone)]
struct ParsedDeclarationSymbol {
    name: String,
    type_display: Option<String>,
    members: Vec<String>,
    member_entries: Vec<LocalMemberEntry>,
    member_source_upper: Option<String>,
    member_source_uppers: Vec<String>,
    member_source_is_rowtype: bool,
    member_source_is_collection_like: bool,
    member_source_allows_visible_members: bool,
    suggest_name: bool,
}

#[derive(Default)]
struct VirtualProjectionMembers {
    columns: Vec<String>,
    rowtype_sources: Vec<String>,
}

struct ResolvedLocalMemberScope {
    members: Vec<String>,
    member_entries: Vec<LocalMemberEntry>,
    member_source_upper: Option<String>,
    member_source_uppers: Vec<String>,
    member_source_is_rowtype: bool,
    member_source_is_collection_like: bool,
}

impl SqlEditorWidget {
    const EXPANDED_STATEMENT_FIRST_WORD_SCAN_BYTES: usize = 1024;

    #[cfg(test)]
    fn expanded_statement_window_in_text(text: &str, cursor_pos: usize) -> ExpandedStatementWindow {
        Self::expanded_statement_window_in_text_for_db_type(text, cursor_pos, None)
    }

    fn expanded_statement_window_in_text_for_db_type(
        text: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> ExpandedStatementWindow {
        if text.is_empty() {
            return ExpandedStatementWindow {
                statement_start: 0,
                statement_end: 0,
                text: String::new(),
                cursor_in_statement: 0,
            };
        }

        let text_len = text.len();
        let cursor_pos = Self::clamp_to_char_boundary_local(text, cursor_pos.min(text_len));
        let mut radius = (INTELLISENSE_STATEMENT_WINDOW as usize)
            .max(1)
            .min(text_len.max(1));

        loop {
            let start = Self::clamp_to_char_boundary_local(text, cursor_pos.saturating_sub(radius));
            let end = Self::clamp_to_char_boundary_local(
                text,
                cursor_pos.saturating_add(radius).min(text_len),
            );
            let window = text.get(start..end).unwrap_or("");
            let rel_cursor = cursor_pos.saturating_sub(start).min(window.len());
            let (stmt_start, stmt_end) =
                Self::statement_bounds_in_text_for_db_type(window, rel_cursor, preferred_db_type);
            let touches_left = stmt_start == 0 && start > 0;
            let touches_right = stmt_end == window.len() && end < text_len;

            if (touches_left && end == text_len) || (touches_right && start == 0) {
                return Self::exact_statement_window_in_text_for_db_type(
                    text,
                    cursor_pos,
                    preferred_db_type,
                );
            }

            if (!touches_left && !touches_right) || (start == 0 && end == text_len) {
                let expanded = Self::statement_window_from_bounds(
                    text,
                    cursor_pos,
                    start.saturating_add(stmt_start),
                    start.saturating_add(stmt_end),
                );
                if Self::expanded_statement_requires_exact_bounds(text, &expanded) {
                    return Self::exact_statement_window_in_text_for_db_type(
                        text,
                        cursor_pos,
                        preferred_db_type,
                    );
                }
                return expanded;
            }

            if radius >= text_len {
                continue;
            }

            let next_radius = radius.saturating_mul(2).min(text_len.max(1));
            if next_radius == radius {
                continue;
            }
            radius = next_radius;
        }
    }

    fn exact_statement_window_in_text_for_db_type(
        text: &str,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> ExpandedStatementWindow {
        let text_len = text.len();
        let cursor_pos = Self::clamp_to_char_boundary_local(text, cursor_pos.min(text_len));
        let (statement_start, statement_end) =
            QueryExecutor::statement_bounds_at_cursor_for_db_type(
                text,
                cursor_pos,
                preferred_db_type,
            )
            .unwrap_or((0, text_len));
        Self::statement_window_from_bounds(text, cursor_pos, statement_start, statement_end)
    }

    fn statement_window_from_bounds(
        text: &str,
        cursor_pos: usize,
        statement_start: usize,
        statement_end: usize,
    ) -> ExpandedStatementWindow {
        let text_len = text.len();
        let statement_start =
            Self::clamp_to_char_boundary_local(text, statement_start.min(text_len));
        let statement_end = Self::clamp_to_char_boundary_local(
            text,
            statement_end.max(statement_start).min(text_len),
        );
        let statement_text = text
            .get(statement_start..statement_end)
            .unwrap_or("")
            .to_string();
        let cursor_in_statement = cursor_pos
            .saturating_sub(statement_start)
            .min(statement_text.len());
        ExpandedStatementWindow {
            statement_start,
            statement_end,
            text: statement_text,
            cursor_in_statement,
        }
    }

    fn expanded_statement_requires_exact_bounds(
        full_text: &str,
        expanded: &ExpandedStatementWindow,
    ) -> bool {
        if expanded.text.is_empty()
            || (expanded.statement_start == 0 && expanded.statement_end == full_text.len())
        {
            return false;
        }

        let first_word_scan_end = Self::clamp_to_char_boundary_local(
            &expanded.text,
            expanded
                .text
                .len()
                .min(Self::EXPANDED_STATEMENT_FIRST_WORD_SCAN_BYTES),
        );
        let first_word = super::query_text::tokenize_sql_spanned(
            expanded.text.get(..first_word_scan_end).unwrap_or(&expanded.text),
        )
            .into_iter()
            .find_map(|span| match span.token {
                SqlToken::Word(word) => Some(word.to_ascii_uppercase()),
                _ => None,
            });

        if matches!(
            first_word.as_deref(),
            Some("PROCEDURE") | Some("FUNCTION") | Some("PACKAGE")
        ) {
            return true;
        }

        if Self::expanded_statement_may_contain_structural_plsql(&expanded.text)
            && Self::expanded_statement_has_structural_plsql_tokens(&expanded.text)
        {
            return true;
        }

        !matches!(
            first_word.as_deref(),
            Some("SELECT")
                | Some("WITH")
                | Some("INSERT")
                | Some("UPDATE")
                | Some("DELETE")
                | Some("MERGE")
                | Some("BEGIN")
                | Some("DECLARE")
                | Some("CREATE")
                | Some("ALTER")
                | Some("DROP")
                | Some("CALL")
                | Some("VALUES")
                | Some("COMMIT")
                | Some("ROLLBACK")
                | Some("SAVEPOINT")
                | Some("PROMPT")
                | Some("VAR")
                | Some("VARIABLE")
                | Some("PRINT")
                | Some("DESCRIBE")
        )
    }

    fn expanded_statement_contains_ascii_case_insensitive(
        haystack: &str,
        needle: &str,
    ) -> bool {
        let haystack_bytes = haystack.as_bytes();
        let needle_bytes = needle.as_bytes();
        if needle_bytes.is_empty() {
            return true;
        }
        if haystack_bytes.len() < needle_bytes.len() {
            return false;
        }

        haystack_bytes
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes))
    }

    fn expanded_statement_may_contain_structural_plsql(text: &str) -> bool {
        Self::expanded_statement_contains_ascii_case_insensitive(text, "BEGIN")
            || Self::expanded_statement_contains_ascii_case_insensitive(text, "DECLARE")
            || Self::expanded_statement_contains_ascii_case_insensitive(text, "PACKAGE")
    }

    fn expanded_statement_has_structural_plsql_tokens(text: &str) -> bool {
        let mut previous_word_was_package = false;
        for span in super::query_text::tokenize_sql_spanned(text) {
            match span.token {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    if matches!(upper.as_str(), "BEGIN" | "DECLARE") {
                        return true;
                    }
                    if previous_word_was_package && upper == "BODY" {
                        return true;
                    }
                    previous_word_was_package = upper == "PACKAGE";
                }
                _ => previous_word_was_package = false,
            }
        }
        false
    }

    fn expanded_statement_window_and_text_binds_from_shadow(
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: usize,
        preferred_db_type: Option<crate::db::connection::DatabaseType>,
    ) -> (ExpandedStatementWindow, Vec<String>) {
        let guard = text_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let expanded =
            Self::expanded_statement_window_in_text_for_db_type(&guard.text, cursor_pos, preferred_db_type);
        let text_bind_names = Self::collect_text_var_bind_names_before_statement(
            &guard.text,
            expanded.statement_start,
        );
        (expanded, text_bind_names)
    }

    fn build_intellisense_analysis_from_routine_cache(
        routine_cache: &RoutineSymbolCacheEntry,
        cursor_in_statement: usize,
    ) -> IntellisenseAnalysis {
        let split_idx = routine_cache
            .token_ends
            .partition_point(|end| *end <= cursor_in_statement);
        let context = intellisense_context::analyze_cursor_context_arc(
            routine_cache.statement_tokens.clone(),
            split_idx,
        );

        let cursor_in_alias_declaration = routine_cache
            .alias_context
            .cursor_within_declaration(cursor_in_statement);

        IntellisenseAnalysis {
            statement_start: routine_cache.statement_start,
            statement_end: routine_cache.statement_end,
            context: Arc::new(context),
            local_scopes: routine_cache.local_scopes.clone(),
            local_symbols: routine_cache.local_symbols.clone(),
            text_bind_names: routine_cache.text_bind_names.clone(),
            cursor_in_alias_declaration,
        }
    }

    fn build_routine_symbol_cache_entry(
        buffer_revision: u64,
        expanded_statement: &ExpandedStatementWindow,
        text_bind_names: Vec<String>,
    ) -> RoutineSymbolCacheEntry {
        let mysql_compatible =
            sql_text::mysql_compatibility_for_sql(&expanded_statement.text, None);
        let token_spans: Vec<SqlTokenSpan> = super::query_text::tokenize_sql_spanned_with_mysql_compat(
            &expanded_statement.text,
            mysql_compatible,
        );
        let (local_scopes, local_symbols) = Self::analyze_local_scopes_and_symbols(
            &expanded_statement.text,
            &token_spans,
            mysql_compatible,
        );
        let alias_context =
            super::query_text::collect_local_alias_context_from_spans(&token_spans);
        let mut statement_tokens = Vec::with_capacity(token_spans.len());
        let mut token_ends = Vec::with_capacity(token_spans.len());
        for span in token_spans {
            token_ends.push(span.end);
            statement_tokens.push(span.token);
        }

        RoutineSymbolCacheEntry {
            buffer_revision,
            statement_start: expanded_statement.statement_start,
            statement_end: expanded_statement.statement_end,
            statement_tokens: statement_tokens.into(),
            token_ends: token_ends.into(),
            local_scopes: local_scopes.into(),
            local_symbols: local_symbols.into(),
            text_bind_names: text_bind_names.into(),
            alias_context: Arc::new(alias_context),
        }
    }

    fn session_bind_names(connection: &SharedConnection) -> Vec<String> {
        // Bind names are an optional enrichment. If the connection mutex is
        // busy (schema refresh or an executing query), skip rather than
        // blocking the UI thread; the next keystroke will retry.
        let session = match connection.try_lock() {
            Ok(guard) => guard.session_state(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned.into_inner().session_state()
            }
            Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
        };

        let names = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .binds
            .keys()
            .cloned()
            .collect();
        names
    }

    fn collect_local_symbol_suggestions(
        prefix: &str,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
        session_bind_names: &[String],
    ) -> Vec<String> {
        let prefix_upper = Self::local_identifier_lookup_upper(prefix);
        let cursor_in_statement = cursor_in_statement.min(
            analysis
                .statement_end
                .saturating_sub(analysis.statement_start),
        );
        let mut suggestions = Vec::new();
        let mut seen_dynamic_symbols = HashSet::new();

        let active_scope = Self::deepest_local_scope_at_cursor(
            analysis.local_scopes.as_ref(),
            cursor_in_statement,
        );
        let scope_chain = Self::local_scope_chain(analysis.local_scopes.as_ref(), active_scope);
        let mut scope_rank_by_id = vec![None; analysis.local_scopes.len()];
        for (rank, scope_id) in scope_chain.iter().copied().enumerate() {
            if let Some(slot) = scope_rank_by_id.get_mut(scope_id) {
                *slot = Some(rank);
            }
        }
        let mut scoped_suggestions = vec![Vec::new(); scope_chain.len()];

        for symbol in analysis.local_symbols.iter() {
            if !symbol.suggest_name || symbol.declared_at > cursor_in_statement {
                continue;
            }
            let Some(scope_rank) = scope_rank_by_id
                .get(symbol.scope_id)
                .and_then(|rank| *rank)
            else {
                continue;
            };
            if !Self::local_symbol_matches_prefix(&symbol.name, &symbol.upper, &prefix_upper) {
                continue;
            }
            scoped_suggestions[scope_rank].push((symbol.upper.clone(), symbol.name.clone()));
        }

        let mut seen_local_symbols = HashSet::new();
        for bucket in scoped_suggestions {
            for (upper, name) in bucket {
                if seen_local_symbols.insert(upper) {
                    suggestions.push(name);
                }
            }
        }

        for name in analysis.text_bind_names.iter() {
            let upper = name.to_ascii_uppercase();
            if !Self::local_symbol_matches_prefix(name, &upper, &prefix_upper) {
                continue;
            }
            if !seen_local_symbols.contains(&upper) && seen_dynamic_symbols.insert(upper) {
                suggestions.push(name.clone());
            }
        }

        for name in session_bind_names {
            let upper = name.to_ascii_uppercase();
            if !Self::local_symbol_matches_prefix(name, &upper, &prefix_upper) {
                continue;
            }
            if !seen_local_symbols.contains(&upper) && seen_dynamic_symbols.insert(upper) {
                suggestions.push(name.clone());
            }
        }

        suggestions
    }

    fn collect_local_record_member_suggestions(
        qualifier: &str,
        prefix: &str,
        cursor_in_statement: usize,
        raw_qualifier: Option<&str>,
        analysis: &IntellisenseAnalysis,
    ) -> Option<Vec<String>> {
        let prefix_upper = Self::local_member_suggestion_lookup_upper(prefix);
        let scope = Self::resolve_local_member_scope_for_qualifier(
            qualifier,
            cursor_in_statement,
            raw_qualifier,
            analysis,
        )?;
        if scope.members.is_empty() {
            return None;
        }

        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        if scope.member_entries.is_empty() {
            for member in &scope.members {
                let upper = Self::local_member_suggestion_lookup_upper(member);
                if !Self::local_symbol_matches_prefix(member, &upper, &prefix_upper) {
                    continue;
                }
                if seen.insert(upper) {
                    suggestions.push(member.clone());
                }
            }
        } else {
            for member in &scope.member_entries {
                if !Self::local_symbol_matches_prefix(&member.name, &member.upper, &prefix_upper) {
                    continue;
                }
                if seen.insert(member.upper.clone()) {
                    suggestions.push(member.name.clone());
                }
            }
        }

        Some(suggestions)
    }

    fn filter_local_symbol_suggestions_by_expected_operand_type(
        suggestions: &mut Vec<String>,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
        expected_type: ExpectedOperandTypes,
    ) {
        suggestions.retain(|suggestion| {
            let Some(symbol) =
                Self::visible_local_symbol_for_qualifier(suggestion, cursor_in_statement, analysis)
            else {
                return true;
            };
            let Some(type_display) = symbol.type_display.as_deref() else {
                return true;
            };

            Self::operand_type_matches_any_expected(
                Self::classify_type_display(type_display),
                expected_type,
            )
        });
    }

    fn current_local_assignment_expected_operand_type(
        tokens: &[SqlToken],
        end: usize,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
    ) -> Option<ExpectedOperandTypes> {
        let assign_idx = Self::previous_non_comment_token_index(tokens, end)?;
        if !matches!(tokens.get(assign_idx), Some(SqlToken::Symbol(sym)) if sym == ":=") {
            return None;
        }
        let target_idx = Self::previous_non_comment_token_index(tokens, assign_idx)?;
        let Some(SqlToken::Word(target_name)) = tokens.get(target_idx) else {
            return None;
        };
        if matches!(
            target_idx.checked_sub(1).and_then(|idx| tokens.get(idx)),
            Some(SqlToken::Symbol(sym)) if sym == "."
        ) {
            return None;
        }

        let symbol =
            Self::visible_local_symbol_for_qualifier(target_name, cursor_in_statement, analysis)?;
        let type_display = symbol.type_display.as_deref()?;
        let operand_type = Self::classify_type_display(type_display);
        match operand_type {
            PrecedingOperandType::Datetime
            | PrecedingOperandType::Character
            | PrecedingOperandType::Numeric
            | PrecedingOperandType::FloatingNumeric
            | PrecedingOperandType::Collection => {
                Some(ExpectedOperandTypes::Single(operand_type))
            }
            PrecedingOperandType::Other | PrecedingOperandType::Unknown => None,
        }
    }

    fn current_declaration_default_expected_operand_type(
        tokens: &[SqlToken],
        end: usize,
    ) -> Option<ExpectedOperandTypes> {
        let anchor_idx = Self::previous_non_comment_token_index(tokens, end)?;
        let is_default_anchor = matches!(
            tokens.get(anchor_idx),
            Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("DEFAULT")
        );
        let is_assignment_anchor =
            matches!(tokens.get(anchor_idx), Some(SqlToken::Symbol(sym)) if sym == ":=");
        if !is_default_anchor && !is_assignment_anchor {
            return None;
        }

        let start_idx = Self::declaration_default_scan_start(tokens, anchor_idx)?;
        let token_spans: Vec<SqlTokenSpan> = tokens[start_idx..anchor_idx]
            .iter()
            .cloned()
            .map(|token| SqlTokenSpan {
                token,
                start: 0,
                end: 0,
            })
            .collect();
        for idx in 0..token_spans.len() {
            if let Some(type_display) = Self::scalar_type_display_at_idx(&token_spans, idx) {
                let operand_type = Self::classify_type_display(&type_display);
                return match operand_type {
                    PrecedingOperandType::Datetime
                    | PrecedingOperandType::Character
                    | PrecedingOperandType::Numeric
                    | PrecedingOperandType::FloatingNumeric
                    | PrecedingOperandType::Collection => {
                        Some(ExpectedOperandTypes::Single(operand_type))
                    }
                    PrecedingOperandType::Other | PrecedingOperandType::Unknown => None,
                };
            }
        }

        None
    }

    fn declaration_default_scan_start(tokens: &[SqlToken], anchor_idx: usize) -> Option<usize> {
        let mut depth = 0i32;
        for idx in (0..anchor_idx.min(tokens.len())).rev() {
            match &tokens[idx] {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    if depth == 0 {
                        return Some(idx.saturating_add(1));
                    }
                    depth -= 1;
                }
                SqlToken::Symbol(sym) if depth == 0 && (sym == ";" || sym == ",") => {
                    return Some(idx.saturating_add(1));
                }
                SqlToken::Word(word)
                    if depth == 0
                        && matches!(
                            word.to_ascii_uppercase().as_str(),
                            "DECLARE" | "IS" | "AS" | "BEGIN"
                        ) =>
                {
                    return Some(idx.saturating_add(1));
                }
                _ => {}
            }
        }

        Some(0)
    }

    fn current_routine_return_expected_operand_type(
        tokens: &[SqlToken],
        end: usize,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
    ) -> Option<ExpectedOperandTypes> {
        let return_idx = Self::previous_non_comment_token_index(tokens, end)?;
        if !matches!(tokens.get(return_idx), Some(SqlToken::Word(word)) if word.eq_ignore_ascii_case("RETURN"))
        {
            return None;
        }

        let scope = analysis
            .local_scopes
            .iter()
            .filter(|scope| {
                matches!(scope.kind, LocalScopeKind::Routine)
                    && cursor_in_statement >= scope.start
                    && cursor_in_statement <= scope.end
                    && scope.return_type_display.is_some()
            })
            .max_by_key(|scope| scope.depth)?;
        let operand_type = Self::classify_type_display(scope.return_type_display.as_deref()?);
        match operand_type {
            PrecedingOperandType::Datetime
            | PrecedingOperandType::Character
            | PrecedingOperandType::Numeric
            | PrecedingOperandType::FloatingNumeric
            | PrecedingOperandType::Collection => {
                Some(ExpectedOperandTypes::Single(operand_type))
            }
            PrecedingOperandType::Other | PrecedingOperandType::Unknown => None,
        }
    }

    fn current_local_into_target_expected_operand_type(
        tokens: &[SqlToken],
        end: usize,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
    ) -> Option<ExpectedOperandTypes> {
        let projection_index = Self::current_select_list_anchor_before_cursor(tokens, end)
            .map(|select_idx| Self::current_select_projection_index(tokens, select_idx, end))
            .or_else(|| {
                Self::current_returning_list_anchor_before_cursor(tokens, end)
                    .map(|returning_idx| {
                        Self::current_returning_projection_index(tokens, returning_idx, end)
                    })
            })?;
        let into_idx = Self::next_into_keyword_after_cursor(tokens, end)?;
        let target_name = Self::nth_identifier_after_into(tokens, into_idx, projection_index)?;
        let symbol =
            Self::visible_local_symbol_for_qualifier(&target_name, cursor_in_statement, analysis)?;
        let type_display = symbol.type_display.as_deref()?;
        let operand_type = Self::classify_type_display(type_display);
        match operand_type {
            PrecedingOperandType::Datetime
            | PrecedingOperandType::Character
            | PrecedingOperandType::Numeric
            | PrecedingOperandType::FloatingNumeric
            | PrecedingOperandType::Collection => {
                Some(ExpectedOperandTypes::Single(operand_type))
            }
            PrecedingOperandType::Other | PrecedingOperandType::Unknown => None,
        }
    }

    fn current_returning_list_anchor_before_cursor(tokens: &[SqlToken], end: usize) -> Option<usize> {
        let mut depth = 0i32;
        for idx in (0..end.min(tokens.len())).rev() {
            match &tokens[idx] {
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => depth += 1,
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                SqlToken::Word(word) if depth == 0 => match word.to_ascii_uppercase().as_str() {
                    "RETURNING" => return Some(idx),
                    "INTO" | "WHERE" | "GROUP" | "HAVING" | "ORDER" | "VALUES" | "SET"
                    | "SELECT" => return None,
                    _ => {}
                },
                SqlToken::Symbol(sym) if depth == 0 && sym == ";" => return None,
                _ => {}
            }
        }
        None
    }

    fn current_returning_projection_index(
        tokens: &[SqlToken],
        returning_idx: usize,
        end: usize,
    ) -> usize {
        let mut projection_index = 0usize;
        let mut depth = 0i32;
        for token in &tokens[(returning_idx + 1)..end.min(tokens.len())] {
            match token {
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => depth += 1,
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                SqlToken::Symbol(sym) if depth == 0 && sym == "," => projection_index += 1,
                _ => {}
            }
        }
        projection_index
    }

    fn next_into_keyword_after_cursor(tokens: &[SqlToken], start: usize) -> Option<usize> {
        let mut depth = 0i32;
        for (idx, token) in tokens.iter().enumerate().skip(start.min(tokens.len())) {
            match token {
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => depth += 1,
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                SqlToken::Word(word) if depth == 0 && word.eq_ignore_ascii_case("INTO") => {
                    return Some(idx);
                }
                SqlToken::Word(word)
                    if depth == 0
                        && matches!(
                            word.to_ascii_uppercase().as_str(),
                            "FROM" | "WHERE" | "GROUP" | "HAVING" | "ORDER" | "UNION"
                                | "INTERSECT" | "EXCEPT" | "MINUS"
                        ) =>
                {
                    return None;
                }
                SqlToken::Symbol(sym) if depth == 0 && sym == ";" => return None,
                _ => {}
            }
        }
        None
    }

    fn nth_identifier_after_into(
        tokens: &[SqlToken],
        into_idx: usize,
        target_index: usize,
    ) -> Option<String> {
        let mut index = 0usize;
        let mut depth = 0i32;
        let mut current_identifier = None;
        let mut idx = into_idx + 1;
        while idx < tokens.len() {
            match &tokens[idx] {
                SqlToken::Word(word)
                    if depth == 0
                        && index == 0
                        && current_identifier.is_none()
                        && word.eq_ignore_ascii_case("BULK") =>
                {
                    idx += 1;
                    if matches!(
                        tokens.get(idx),
                        Some(SqlToken::Word(next_word)) if next_word.eq_ignore_ascii_case("COLLECT")
                    ) {
                        idx += 1;
                    }
                    continue;
                }
                SqlToken::Symbol(sym) if sym == "(" || sym == "[" => depth += 1,
                SqlToken::Symbol(sym) if sym == ")" || sym == "]" => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                SqlToken::Symbol(sym) if depth == 0 && sym == "," => {
                    if index == target_index {
                        return current_identifier;
                    }
                    index += 1;
                    current_identifier = None;
                }
                SqlToken::Word(word)
                    if depth == 0
                        && matches!(
                            word.to_ascii_uppercase().as_str(),
                            "FROM" | "WHERE" | "GROUP" | "HAVING" | "ORDER" | "UNION"
                                | "INTERSECT" | "EXCEPT" | "MINUS"
                        ) =>
                {
                    break;
                }
                SqlToken::Word(word) if depth == 0 => {
                    if matches!(
                        idx.checked_sub(1).and_then(|prev_idx| tokens.get(prev_idx)),
                        Some(SqlToken::Symbol(sym)) if sym == "." || sym == ":"
                    ) {
                        return None;
                    }
                    current_identifier = Some(word.clone());
                }
                SqlToken::Symbol(sym) if depth == 0 && sym == ";" => break,
                _ => {}
            }
            idx += 1;
        }

        (index == target_index).then_some(current_identifier).flatten()
    }

    fn filter_local_record_member_suggestions_by_expected_operand_type(
        suggestions: &mut Vec<String>,
        qualifier: &str,
        cursor_in_statement: usize,
        raw_qualifier: Option<&str>,
        analysis: &IntellisenseAnalysis,
        expected_type: ExpectedOperandTypes,
    ) {
        let Some(scope) = Self::resolve_local_member_scope_for_qualifier(
            qualifier,
            cursor_in_statement,
            raw_qualifier,
            analysis,
        ) else {
            return;
        };

        suggestions.retain(|suggestion| {
            let suggestion_upper = Self::local_member_suggestion_lookup_upper(suggestion);
            let Some(member) = scope
                .member_entries
                .iter()
                .find(|entry| entry.upper == suggestion_upper)
            else {
                return true;
            };
            let Some(type_display) = member.type_display.as_deref() else {
                return true;
            };

            Self::operand_type_matches_any_expected(
                Self::classify_type_display(type_display),
                expected_type,
            )
        });
    }

    fn local_member_suggestion_lookup_upper(member: &str) -> String {
        let trimmed = member.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            sql_text::strip_identifier_quotes(trimmed).to_ascii_uppercase()
        } else if matches!(trimmed.chars().next(), Some('"') | Some('`') | Some('[')) {
            trimmed[1..].to_ascii_uppercase()
        } else {
            member.to_ascii_uppercase()
        }
    }

    fn local_rowtype_member_sources_for_qualifier(
        qualifier: &str,
        cursor_in_statement: usize,
        raw_qualifier: Option<&str>,
        analysis: &IntellisenseAnalysis,
    ) -> Vec<String> {
        let scope = Self::resolve_local_member_scope_for_qualifier(
            qualifier,
            cursor_in_statement,
            raw_qualifier,
            analysis,
        );
        let Some(scope) = scope else {
            return Vec::new();
        };
        if !scope.member_source_is_rowtype {
            return Vec::new();
        }

        let mut sources = scope.member_source_uppers.clone();
        if sources.is_empty() {
            sources.extend(scope.member_source_upper.iter().cloned());
        }
        Self::dedup_column_names_case_insensitive(&mut sources);
        sources
    }

    fn resolve_local_member_scope_for_qualifier(
        qualifier: &str,
        cursor_in_statement: usize,
        raw_qualifier: Option<&str>,
        analysis: &IntellisenseAnalysis,
    ) -> Option<ResolvedLocalMemberScope> {
        let segments = Self::local_qualifier_segments(qualifier, raw_qualifier);
        let segment_indexed =
            Self::local_qualifier_indexed_segment_flags(segments.len(), raw_qualifier);
        let base_qualifier = segments.first()?;
        let base_symbol =
            Self::visible_local_symbol_for_qualifier(base_qualifier, cursor_in_statement, analysis)?;
        let mut scope = Self::resolved_scope_from_local_symbol(base_symbol);
        if segment_indexed.first().copied().unwrap_or(false) && !scope.member_source_is_collection_like {
            return None;
        }

        for (segment_idx, segment) in segments.iter().skip(1).enumerate() {
            let segment_upper = segment.to_ascii_uppercase();
            let member = scope
                .member_entries
                .iter()
                .find(|entry| entry.upper == segment_upper)?;
            scope = Self::resolve_local_member_entry_scope(
                member,
                base_symbol.scope_id,
                cursor_in_statement,
                analysis,
            )?;
            if segment_indexed
                .get(segment_idx.saturating_add(1))
                .copied()
                .unwrap_or(false)
                && !scope.member_source_is_collection_like
            {
                return None;
            }
        }

        Some(scope)
    }

    fn local_qualifier_segments(qualifier: &str, raw_qualifier: Option<&str>) -> Vec<String> {
        if let Some(raw_qualifier) = raw_qualifier {
            let segments = Self::split_raw_qualifier_segments(raw_qualifier)
                .into_iter()
                .map(Self::normalize_raw_qualifier_segment)
                .collect::<Option<Vec<_>>>();
            if let Some(segments) = segments.filter(|segments| !segments.is_empty()) {
                return segments;
            }
        }

        qualifier.split('.').map(ToString::to_string).collect()
    }

    fn local_qualifier_indexed_segment_flags(
        segment_count: usize,
        raw_qualifier: Option<&str>,
    ) -> Vec<bool> {
        let mut flags = vec![false; segment_count];
        let Some(raw_qualifier) = raw_qualifier else {
            return flags;
        };

        let raw_segments = Self::split_raw_qualifier_segments(raw_qualifier);
        if raw_segments.len() == segment_count {
            for (flag, raw_segment) in flags.iter_mut().zip(raw_segments) {
                *flag = raw_segment.trim_end().ends_with(')');
            }
        } else if raw_qualifier.trim_end().ends_with(')') {
            if let Some(last) = flags.last_mut() {
                *last = true;
            }
        }

        flags
    }

    fn normalize_raw_qualifier_segment(raw_segment: &str) -> Option<String> {
        let raw_segment = raw_segment.trim();
        if raw_segment.is_empty() {
            return None;
        }

        let mut segment_end = raw_segment.len();
        if raw_segment.get(..segment_end)?.trim_end().ends_with(')') {
            segment_end = Self::find_open_paren_for_qualifier_expression(raw_segment, segment_end)?;
        }
        let (segment, segment_start) =
            Self::parse_qualifier_segment_before_dot(raw_segment, segment_end)?;
        if segment_start != 0 {
            return None;
        }
        Some(segment)
    }

    fn split_raw_qualifier_segments(raw_qualifier: &str) -> Vec<&str> {
        let mut segments = Vec::new();
        let mut segment_start = 0usize;
        let mut paren_depth = 0usize;
        let mut active_quote = None::<char>;
        let mut chars = raw_qualifier.char_indices().peekable();

        while let Some((idx, ch)) = chars.next() {
            if let Some(quote) = active_quote {
                if ch == quote {
                    if chars.peek().is_some_and(|(_, next)| *next == quote) {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '\'' | '"' | '`' => active_quote = Some(ch),
                '[' => active_quote = Some(']'),
                '(' => paren_depth = paren_depth.saturating_add(1),
                ')' => paren_depth = paren_depth.saturating_sub(1),
                '.' if paren_depth == 0 => {
                    if let Some(segment) = raw_qualifier.get(segment_start..idx) {
                        segments.push(segment);
                    }
                    segment_start = idx + ch.len_utf8();
                }
                _ => {}
            }
        }

        if let Some(segment) = raw_qualifier.get(segment_start..) {
            segments.push(segment);
        }
        segments
    }

    fn resolved_scope_from_local_symbol(symbol: &LocalSymbolEntry) -> ResolvedLocalMemberScope {
        ResolvedLocalMemberScope {
            members: symbol.members.clone(),
            member_entries: symbol.member_entries.clone(),
            member_source_upper: symbol.member_source_upper.clone(),
            member_source_uppers: symbol.member_source_uppers.clone(),
            member_source_is_rowtype: symbol.member_source_is_rowtype,
            member_source_is_collection_like: symbol.member_source_is_collection_like,
        }
    }

    fn resolve_local_member_entry_scope(
        member: &LocalMemberEntry,
        scope_id: usize,
        cursor_in_statement: usize,
        analysis: &IntellisenseAnalysis,
    ) -> Option<ResolvedLocalMemberScope> {
        if member.member_source_is_rowtype {
            return Some(ResolvedLocalMemberScope {
                members: Vec::new(),
                member_entries: Vec::new(),
                member_source_upper: member.member_source_upper.clone(),
                member_source_uppers: member.member_source_uppers.clone(),
                member_source_is_rowtype: true,
                member_source_is_collection_like: member.member_source_is_collection_like,
            });
        }

        let source_upper = member.member_source_upper.as_deref()?;
        let source = Self::visible_local_symbol_for_source_upper(
            source_upper,
            scope_id,
            cursor_in_statement,
            member.member_source_allows_visible_members,
            analysis,
        )?;
        Some(Self::resolved_scope_from_local_symbol(source))
    }

    fn visible_local_symbol_for_qualifier<'a>(
        qualifier: &str,
        cursor_in_statement: usize,
        analysis: &'a IntellisenseAnalysis,
    ) -> Option<&'a LocalSymbolEntry> {
        let qualifier_upper = Self::local_identifier_lookup_upper(qualifier);
        let cursor_in_statement = cursor_in_statement.min(
            analysis
                .statement_end
                .saturating_sub(analysis.statement_start),
        );

        let active_scope = Self::deepest_local_scope_at_cursor(
            analysis.local_scopes.as_ref(),
            cursor_in_statement,
        );
        let scope_chain = Self::local_scope_chain(analysis.local_scopes.as_ref(), active_scope);
        let mut scope_rank_by_id = vec![None; analysis.local_scopes.len()];
        for (rank, scope_id) in scope_chain.iter().copied().enumerate() {
            if let Some(slot) = scope_rank_by_id.get_mut(scope_id) {
                *slot = Some(rank);
            }
        }

        let mut best: Option<(usize, usize, &LocalSymbolEntry)> = None;
        for symbol in analysis.local_symbols.iter() {
            if !symbol.suggest_name
                || symbol.declared_at > cursor_in_statement
                || symbol.upper != qualifier_upper
            {
                continue;
            }
            let Some(scope_rank) = scope_rank_by_id
                .get(symbol.scope_id)
                .and_then(|rank| *rank)
            else {
                continue;
            };
            if best.is_none_or(|(best_rank, best_declared_at, _)| {
                scope_rank < best_rank
                    || (scope_rank == best_rank && symbol.declared_at >= best_declared_at)
            }) {
                best = Some((scope_rank, symbol.declared_at, symbol));
            }
        }

        best.map(|(_, _, symbol)| symbol)
    }

    fn visible_local_symbol_for_source_upper<'a>(
        source_upper: &str,
        scope_id: usize,
        cursor_in_statement: usize,
        allow_visible_symbols: bool,
        analysis: &'a IntellisenseAnalysis,
    ) -> Option<&'a LocalSymbolEntry> {
        let scope_chain = Self::local_scope_chain(analysis.local_scopes.as_ref(), scope_id);
        let mut scope_rank_by_id = vec![None; analysis.local_scopes.len()];
        for (rank, scope_id) in scope_chain.iter().copied().enumerate() {
            if let Some(slot) = scope_rank_by_id.get_mut(scope_id) {
                *slot = Some(rank);
            }
        }

        let mut best: Option<(usize, usize, &LocalSymbolEntry)> = None;
        for symbol in analysis.local_symbols.iter() {
            if symbol.declared_at > cursor_in_statement
                || symbol.upper != source_upper
                || (symbol.suggest_name && !allow_visible_symbols)
            {
                continue;
            }
            let Some(scope_rank) = scope_rank_by_id
                .get(symbol.scope_id)
                .and_then(|rank| *rank)
            else {
                continue;
            };
            if best.is_none_or(|(best_rank, best_declared_at, _)| {
                scope_rank < best_rank
                    || (scope_rank == best_rank && symbol.declared_at >= best_declared_at)
            }) {
                best = Some((scope_rank, symbol.declared_at, symbol));
            }
        }

        best.map(|(_, _, symbol)| symbol)
    }

    fn prepend_local_symbol_suggestions(base: Vec<String>, locals: Vec<String>) -> Vec<String> {
        if locals.is_empty() {
            let mut base = base;
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let mut merged = Vec::with_capacity(locals.len().saturating_add(base.len()));
        merged.extend(locals);
        merged.extend(base);
        let mut seen = HashSet::new();
        merged.retain(|value| seen.insert(Self::local_identifier_lookup_upper(value)));
        merged.truncate(MAX_MERGED_SUGGESTIONS);
        merged
    }

    fn local_symbol_matches_prefix(_name: &str, upper: &str, prefix_upper: &str) -> bool {
        if prefix_upper.is_empty() {
            return true;
        }

        upper.starts_with(prefix_upper) && upper != prefix_upper
    }

    fn deepest_local_scope_at_cursor(scopes: &[LocalScope], cursor_byte: usize) -> usize {
        let mut best_idx = 0usize;
        let mut best_depth = 0usize;

        for (idx, scope) in scopes.iter().enumerate() {
            if !Self::local_scope_contains(scope, cursor_byte) {
                continue;
            }
            if scope.depth >= best_depth {
                best_depth = scope.depth;
                best_idx = idx;
            }
        }

        best_idx
    }

    fn local_scope_chain(scopes: &[LocalScope], mut scope_id: usize) -> Vec<usize> {
        let mut chain = Vec::new();
        loop {
            chain.push(scope_id);
            let Some(parent) = scopes.get(scope_id).and_then(|scope| scope.parent) else {
                break;
            };
            scope_id = parent;
        }
        chain
    }

    fn local_scope_contains(scope: &LocalScope, cursor_byte: usize) -> bool {
        cursor_byte >= scope.start && cursor_byte <= scope.end
    }

    fn analyze_local_scopes_and_symbols(
        statement_text: &str,
        token_spans: &[SqlTokenSpan],
        mysql_compatible: bool,
    ) -> (Vec<LocalScope>, Vec<LocalSymbolEntry>) {
        let statement_len = statement_text.len();
        let root_begins_with_begin = token_spans.iter().find_map(|span| match &span.token {
            SqlToken::Word(word) => Some(word.eq_ignore_ascii_case("BEGIN")),
            SqlToken::Comment(_) => None,
            _ => Some(false),
        }) == Some(true);
        let mut scopes = vec![LocalScopeBuilder {
            scope: LocalScope {
                parent: None,
                start: 0,
                end: statement_len,
                depth: 0,
                kind: LocalScopeKind::Statement,
                return_type_display: None,
            },
            token_start_idx: 0,
            token_end_idx: token_spans.len(),
            decl_start_idx: None,
            decl_end_idx: None,
            mysql_declare_statements: mysql_compatible && root_begins_with_begin,
        }];
        let mut symbols = Vec::new();
        let mut seen_symbol_keys = HashSet::new();
        let mut block_stack = Vec::<LocalBlockFrame>::new();
        let mut root_decl_start_idx = None;
        let mut root_decl_end_idx = None;
        let mut root_awaiting_body_begin = false;
        let mut pending_loop_var = None::<ParsedForLoopRecord>;
        let mut skip_token_idx = None::<usize>;
        let mut idx = 0usize;
        let previous_meaningful_word_is_end =
            Self::previous_meaningful_word_is_end(token_spans);

        while idx < token_spans.len() {
            if skip_token_idx == Some(idx) {
                idx += 1;
                continue;
            }

            let token = &token_spans[idx];
            let prev_is_end = previous_meaningful_word_is_end
                .get(idx)
                .copied()
                .unwrap_or(false);

            match &token.token {
                SqlToken::Comment(_) | SqlToken::String(_) => {}
                SqlToken::Symbol(sym) if sym == ";" => {
                    pending_loop_var = None;
                }
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();

                    if upper == "PACKAGE"
                        && root_decl_start_idx.is_none()
                        && !prev_is_end
                    {
                        if let Some(parsed) = Self::parse_package_body_header(token_spans, idx) {
                            scopes[0].scope.kind = LocalScopeKind::PackageBody;
                            root_decl_start_idx = Some(parsed.decl_start_idx);
                            root_awaiting_body_begin = true;
                            idx = parsed.body_keyword_idx.saturating_add(1);
                            continue;
                        }
                    }

                    if matches!(upper.as_str(), "PROCEDURE" | "FUNCTION")
                        && !prev_is_end
                    {
                        if let Some(parsed) = Self::parse_routine_header(token_spans, idx) {
                            let parent_scope = Self::current_local_parent_scope_id(&block_stack);
                            let scope_depth = scopes[parent_scope].scope.depth.saturating_add(1);
                            let scope_id = scopes.len();
                            let scope_start = token_spans
                                .get(parsed.body_keyword_idx)
                                .map(|span| span.end)
                                .unwrap_or(token.end);
                            scopes.push(LocalScopeBuilder {
                                scope: LocalScope {
                                    parent: Some(parent_scope),
                                    start: scope_start,
                                    end: statement_len,
                                    depth: scope_depth,
                                    kind: LocalScopeKind::Routine,
                                    return_type_display: parsed.return_type_display.clone(),
                                },
                                token_start_idx: idx,
                                token_end_idx: token_spans.len(),
                                decl_start_idx: Some(parsed.decl_start_idx),
                                decl_end_idx: if parsed.body_starts_immediately {
                                    Some(parsed.body_keyword_idx)
                                } else {
                                    None
                                },
                                mysql_declare_statements: parsed.body_starts_immediately,
                            });
                            for parameter in parsed.parameters {
                                Self::push_local_symbol_with_metadata_and_sources(
                                    &mut symbols,
                                    &mut seen_symbol_keys,
                                    scope_id,
                                    parameter.name,
                                    scope_start,
                                    parameter.type_display,
                                    parameter.members,
                                    parameter.member_entries,
                                    parameter.member_source_upper,
                                    parameter.member_source_uppers,
                                    parameter.member_source_is_rowtype,
                                    parameter.member_source_is_collection_like,
                                    parameter.member_source_allows_visible_members,
                                    parameter.suggest_name,
                                );
                            }
                            block_stack.push(LocalBlockFrame {
                                kind: LocalBlockKind::Routine,
                                scope_id: Some(scope_id),
                                awaiting_body_begin: !parsed.body_starts_immediately,
                            });
                            idx = parsed.body_keyword_idx.saturating_add(1);
                            continue;
                        }
                    }

                    match upper.as_str() {
                        "DECLARE" => {
                            if Self::current_scope_uses_mysql_declare_statements(&scopes, &block_stack)
                            {
                                let scope_id = Self::current_local_parent_scope_id(&block_stack);
                                let item_end =
                                    Self::find_statement_item_end(token_spans, idx, token_spans.len());
                                let item = &token_spans[idx..item_end];
                                let declared_at = Self::declaration_item_declared_at(item);
                                for declaration in
                                    Self::extract_mysql_declaration_symbols_from_item(item)
                                {
                                    Self::push_local_symbol_with_metadata_and_sources(
                                        &mut symbols,
                                        &mut seen_symbol_keys,
                                        scope_id,
                                        declaration.name,
                                        declared_at,
                                        declaration.type_display,
                                        declaration.members,
                                        declaration.member_entries,
                                        declaration.member_source_upper,
                                        declaration.member_source_uppers,
                                        declaration.member_source_is_rowtype,
                                        declaration.member_source_is_collection_like,
                                        declaration.member_source_allows_visible_members,
                                        declaration.suggest_name,
                                    );
                                }
                                idx = item_end.saturating_sub(1);
                                continue;
                            }

                            let parent_scope = Self::current_local_parent_scope_id(&block_stack);
                            let scope_depth = scopes[parent_scope].scope.depth.saturating_add(1);
                            let scope_id = scopes.len();
                            scopes.push(LocalScopeBuilder {
                                scope: LocalScope {
                                    parent: Some(parent_scope),
                                    start: token.start,
                                    end: statement_len,
                                    depth: scope_depth,
                                    kind: LocalScopeKind::DeclareBlock,
                                    return_type_display: None,
                                },
                                token_start_idx: idx,
                                token_end_idx: token_spans.len(),
                                decl_start_idx: Some(idx.saturating_add(1)),
                                decl_end_idx: None,
                                mysql_declare_statements: false,
                            });
                            block_stack.push(LocalBlockFrame {
                                kind: LocalBlockKind::Declare,
                                scope_id: Some(scope_id),
                                awaiting_body_begin: true,
                            });
                        }
                        "FOR" => {
                            if pending_loop_var.is_none() {
                                pending_loop_var = Self::parse_for_loop_record(token_spans, idx);
                            }
                        }
                        "LOOP" => {
                            if prev_is_end {
                                idx += 1;
                                continue;
                            }

                            let scope_id = pending_loop_var.take().map(|record| {
                                let parent_scope =
                                    Self::current_local_parent_scope_id(&block_stack);
                                let scope_depth =
                                    scopes[parent_scope].scope.depth.saturating_add(1);
                                let next_scope_id = scopes.len();
                                scopes.push(LocalScopeBuilder {
                                    scope: LocalScope {
                                        parent: Some(parent_scope),
                                        start: token.end,
                                        end: statement_len,
                                        depth: scope_depth,
                                        kind: LocalScopeKind::Loop,
                                        return_type_display: None,
                                    },
                                    token_start_idx: idx,
                                    token_end_idx: token_spans.len(),
                                    decl_start_idx: None,
                                    decl_end_idx: None,
                                    mysql_declare_statements: Self::scope_uses_mysql_declare_statements(
                                        &scopes,
                                        parent_scope,
                                    ),
                                });
                                Self::push_local_symbol_with_metadata_and_sources(
                                    &mut symbols,
                                    &mut seen_symbol_keys,
                                    next_scope_id,
                                    record.name,
                                    token.end,
                                    None,
                                    record.members,
                                    Vec::new(),
                                    record.member_source_upper,
                                    record.member_source_uppers,
                                    record.member_source_is_rowtype,
                                    false,
                                    true,
                                    true,
                                );
                                next_scope_id
                            });
                            block_stack.push(LocalBlockFrame {
                                kind: LocalBlockKind::Loop,
                                scope_id,
                                awaiting_body_begin: false,
                            });
                        }
                        "IF" => {
                            if !prev_is_end {
                                block_stack.push(LocalBlockFrame {
                                    kind: LocalBlockKind::If,
                                    scope_id: None,
                                    awaiting_body_begin: false,
                                });
                            }
                        }
                        "CASE" => {
                            if !prev_is_end {
                                block_stack.push(LocalBlockFrame {
                                    kind: LocalBlockKind::Case,
                                    scope_id: None,
                                    awaiting_body_begin: false,
                                });
                            }
                        }
                        "BEGIN" => {
                            if let Some(frame) = block_stack.last_mut() {
                                if frame.awaiting_body_begin
                                    && matches!(
                                        frame.kind,
                                        LocalBlockKind::Routine | LocalBlockKind::Declare
                                    )
                                {
                                    if let Some(scope_id) = frame.scope_id {
                                        scopes[scope_id].decl_end_idx = Some(idx);
                                    }
                                    frame.awaiting_body_begin = false;
                                    idx += 1;
                                    continue;
                                }
                            }

                            if root_awaiting_body_begin && block_stack.is_empty() {
                                root_decl_end_idx = Some(idx);
                                root_awaiting_body_begin = false;
                                idx += 1;
                                continue;
                            }

                            let begin_scope_id =
                                if Self::current_scope_uses_mysql_declare_statements(
                                    &scopes,
                                    &block_stack,
                                ) {
                                    let parent_scope =
                                        Self::current_local_parent_scope_id(&block_stack);
                                    let scope_depth =
                                        scopes[parent_scope].scope.depth.saturating_add(1);
                                    let scope_id = scopes.len();
                                    scopes.push(LocalScopeBuilder {
                                        scope: LocalScope {
                                            parent: Some(parent_scope),
                                            start: token.end,
                                            end: statement_len,
                                            depth: scope_depth,
                                            kind: LocalScopeKind::Block,
                                            return_type_display: None,
                                        },
                                        token_start_idx: idx,
                                        token_end_idx: token_spans.len(),
                                        decl_start_idx: None,
                                        decl_end_idx: None,
                                        mysql_declare_statements:
                                            Self::scope_uses_mysql_declare_statements(
                                                &scopes,
                                                parent_scope,
                                            ),
                                    });
                                    Some(scope_id)
                                } else {
                                    None
                                };

                            block_stack.push(LocalBlockFrame {
                                kind: LocalBlockKind::Begin,
                                scope_id: begin_scope_id,
                                awaiting_body_begin: false,
                            });
                        }
                        "END" => {
                            let suffix_idx = Self::next_meaningful_token_idx(token_spans, idx + 1);
                            let suffix_upper = suffix_idx
                                .and_then(|next_idx| Self::token_word(&token_spans[next_idx].token))
                                .map(|word| word.to_ascii_uppercase());

                            match suffix_upper.as_deref() {
                                Some("IF") => {
                                    Self::pop_local_block_kind(
                                        &mut block_stack,
                                        &mut scopes,
                                        LocalBlockKind::If,
                                        token.start,
                                        idx.saturating_add(1),
                                    );
                                    skip_token_idx = suffix_idx;
                                }
                                Some("LOOP") => {
                                    Self::pop_local_block_kind(
                                        &mut block_stack,
                                        &mut scopes,
                                        LocalBlockKind::Loop,
                                        token.start,
                                        idx.saturating_add(1),
                                    );
                                    skip_token_idx = suffix_idx;
                                    pending_loop_var = None;
                                }
                                Some("CASE") => {
                                    Self::pop_local_block_kind(
                                        &mut block_stack,
                                        &mut scopes,
                                        LocalBlockKind::Case,
                                        token.start,
                                        idx.saturating_add(1),
                                    );
                                    skip_token_idx = suffix_idx;
                                }
                                _ => {
                                    if !block_stack.is_empty() {
                                        Self::pop_local_block(
                                            &mut block_stack,
                                            &mut scopes,
                                            token.start,
                                            idx.saturating_add(1),
                                        );
                                    } else if root_decl_end_idx.is_none()
                                        && matches!(
                                            scopes[0].scope.kind,
                                            LocalScopeKind::PackageBody
                                        )
                                    {
                                        root_decl_end_idx = Some(idx);
                                        root_awaiting_body_begin = false;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }

            idx += 1;
        }

        if root_decl_start_idx.is_some() && root_decl_end_idx.is_none() {
            root_decl_end_idx = Some(token_spans.len());
        }

        if let Some(root_decl_end_idx) = root_decl_end_idx {
            scopes[0].decl_start_idx = root_decl_start_idx;
            scopes[0].decl_end_idx = Some(root_decl_end_idx);
        }

        while let Some(frame) = block_stack.pop() {
            if let Some(scope_id) = frame.scope_id {
                scopes[scope_id].scope.end = statement_len;
                scopes[scope_id].token_end_idx = token_spans.len();
                if scopes[scope_id].decl_end_idx.is_none() && frame.awaiting_body_begin {
                    scopes[scope_id].decl_end_idx = Some(token_spans.len());
                }
            }
        }

        let mut child_ranges_by_scope = vec![Vec::new(); scopes.len()];
        for scope in &scopes {
            if let Some(parent_scope) = scope.scope.parent {
                child_ranges_by_scope[parent_scope]
                    .push((scope.token_start_idx, scope.token_end_idx));
            }
        }

        for (scope_id, scope) in scopes.iter().enumerate() {
            let Some(decl_start_idx) = scope.decl_start_idx else {
                continue;
            };
            let Some(decl_end_idx) = scope.decl_end_idx else {
                continue;
            };
            if decl_start_idx >= decl_end_idx || decl_start_idx >= token_spans.len() {
                continue;
            }
            Self::collect_scope_declaration_symbols(
                scope_id,
                token_spans,
                decl_start_idx,
                decl_end_idx.min(token_spans.len()),
                child_ranges_by_scope
                    .get(scope_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &mut symbols,
                &mut seen_symbol_keys,
            );
        }

        let scopes: Vec<LocalScope> = scopes.into_iter().map(|builder| builder.scope).collect();
        Self::resolve_deferred_local_record_members(&scopes, &mut symbols);
        (scopes, symbols)
    }

    fn previous_meaningful_word_is_end(token_spans: &[SqlTokenSpan]) -> Vec<bool> {
        let mut previous_word_is_end = Vec::with_capacity(token_spans.len());
        let mut is_end = false;

        for token in token_spans {
            previous_word_is_end.push(is_end);
            match &token.token {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => is_end = word.eq_ignore_ascii_case("END"),
                _ => is_end = false,
            }
        }

        previous_word_is_end
    }

    fn collect_scope_declaration_symbols(
        scope_id: usize,
        token_spans: &[SqlTokenSpan],
        decl_start_idx: usize,
        decl_end_idx: usize,
        child_ranges: &[(usize, usize)],
        symbols: &mut Vec<LocalSymbolEntry>,
        seen_symbol_keys: &mut HashSet<(usize, usize, String)>,
    ) {
        let mut child_idx = 0usize;
        let mut idx = decl_start_idx;
        while idx < decl_end_idx {
            while child_idx < child_ranges.len() && child_ranges[child_idx].1 <= idx {
                child_idx += 1;
            }
            if child_idx < child_ranges.len() && idx >= child_ranges[child_idx].0 {
                idx = child_ranges[child_idx].1.min(decl_end_idx);
                continue;
            }

            let Some(item_start) = Self::next_meaningful_token_idx(token_spans, idx) else {
                break;
            };
            if item_start >= decl_end_idx {
                break;
            }
            if child_idx < child_ranges.len() && item_start >= child_ranges[child_idx].0 {
                idx = child_ranges[child_idx].1.min(decl_end_idx);
                continue;
            }

            let mut item_end = item_start;
            if child_idx < child_ranges.len() && item_end == child_ranges[child_idx].0 {
                break;
            }
            item_end = Self::find_statement_item_end(token_spans, item_start, decl_end_idx);

            if item_end <= item_start {
                idx = idx.saturating_add(1);
                continue;
            }

            let item = &token_spans[item_start..item_end];
            if let Some(declaration) = Self::extract_declaration_symbol_from_item(item) {
                let declared_at = Self::declaration_item_declared_at(item);
                Self::push_local_symbol_with_metadata_and_sources(
                    symbols,
                    seen_symbol_keys,
                    scope_id,
                    declaration.name,
                    declared_at,
                    declaration.type_display,
                    declaration.members,
                    declaration.member_entries,
                    declaration.member_source_upper,
                    declaration.member_source_uppers,
                    declaration.member_source_is_rowtype,
                    declaration.member_source_is_collection_like,
                    declaration.member_source_allows_visible_members,
                    declaration.suggest_name,
                );
            }

            idx = item_end;
        }
    }

    fn extract_declaration_symbol_from_item(item: &[SqlTokenSpan]) -> Option<ParsedDeclarationSymbol> {
        let first_idx = item
            .iter()
            .position(|span| !matches!(span.token, SqlToken::Comment(_)))?;
        let first_word = Self::token_word(&item[first_idx].token)?;
        let first_upper = first_word.to_ascii_uppercase();

        match first_upper.as_str() {
            "PROCEDURE" | "FUNCTION" | "SUBTYPE" | "PRAGMA" | "EXCEPTION" => {
                return None;
            }
            "TYPE" => {
                return Self::extract_record_type_symbol_from_item(item, first_idx)
                    .or_else(|| Self::extract_collection_type_symbol_from_item(item, first_idx));
            }
            "CURSOR" => {
                let cursor_name = item[first_idx + 1..]
                    .iter()
                    .find_map(|span| Self::token_word(&span.token))
                    .and_then(Self::local_identifier_suggestion_from_word)?;
                let (members, member_source_uppers) =
                    Self::extract_cursor_projection_members_and_source_from_item(item, first_idx);
                let member_source_upper = member_source_uppers.first().cloned();
                let member_source_is_rowtype = !member_source_uppers.is_empty();
                return Some(ParsedDeclarationSymbol {
                    name: cursor_name,
                    type_display: None,
                    members,
                    member_entries: Vec::new(),
                    member_source_upper,
                    member_source_uppers,
                    member_source_is_rowtype,
                    member_source_is_collection_like: false,
                    member_source_allows_visible_members: member_source_is_rowtype,
                    suggest_name: true,
                });
            }
            _ => {}
        }

        let name = Self::local_identifier_suggestion_from_word(first_word)?;

        let next_meaningful = item[first_idx + 1..]
            .iter()
            .find(|span| !matches!(span.token, SqlToken::Comment(_)));
        if let Some(next_token) = next_meaningful {
            if Self::token_symbol_is(&next_token.token, ":=")
                || Self::token_symbol_is(&next_token.token, ".")
            {
                return None;
            }
        }

        let (member_source_upper, member_source_is_rowtype) =
            Self::extract_declaration_member_source(item, first_idx);
        let type_display = Self::extract_declaration_scalar_type_display(item, first_idx);
        let member_source_uppers =
            Self::rowtype_source_uppers_from_single(&member_source_upper, member_source_is_rowtype);
        Some(ParsedDeclarationSymbol {
            name,
            type_display,
            members: Vec::new(),
            member_entries: Vec::new(),
            member_source_upper,
            member_source_uppers,
            member_source_is_rowtype,
            member_source_is_collection_like: false,
            member_source_allows_visible_members: member_source_is_rowtype,
            suggest_name: true,
        })
    }

    fn extract_record_type_symbol_from_item(
        item: &[SqlTokenSpan],
        type_keyword_idx: usize,
    ) -> Option<ParsedDeclarationSymbol> {
        let name_idx = Self::next_meaningful_token_idx(item, type_keyword_idx + 1)?;
        let name =
            Self::token_word(&item[name_idx].token).and_then(Self::local_identifier_from_word)?;

        let mut depth = 0usize;
        let mut idx = name_idx + 1;
        let mut record_keyword_idx = None;
        while idx < item.len() {
            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
                SqlToken::Word(word)
                    if depth == 0
                        && (word.eq_ignore_ascii_case("IS") || word.eq_ignore_ascii_case("AS")) =>
                {
                    let next_idx = Self::next_meaningful_token_idx(item, idx + 1)?;
                    let next_word = Self::token_word(&item[next_idx].token)?;
                    if !next_word.eq_ignore_ascii_case("RECORD") {
                        return None;
                    }
                    record_keyword_idx = Some(next_idx);
                    break;
                }
                _ => {}
            }
            idx += 1;
        }

        let record_keyword_idx = record_keyword_idx?;
        let open_idx = Self::next_meaningful_token_idx(item, record_keyword_idx + 1)?;
        if !Self::token_symbol_at(item, open_idx, "(") {
            return None;
        }
        let close_idx = Self::matching_local_paren_idx(item, open_idx)?;
        let (mut members, mut member_entries) =
            Self::extract_record_type_fields(item, open_idx, close_idx);
        Self::dedup_column_names_case_insensitive(&mut members);
        Self::dedup_local_member_entries_case_insensitive(&mut member_entries);
        if members.is_empty() {
            return None;
        }

        Some(ParsedDeclarationSymbol {
            name,
            type_display: None,
            members,
            member_entries,
            member_source_upper: None,
            member_source_uppers: Vec::new(),
            member_source_is_rowtype: false,
            member_source_is_collection_like: false,
            member_source_allows_visible_members: false,
            suggest_name: false,
        })
    }

    fn extract_collection_type_symbol_from_item(
        item: &[SqlTokenSpan],
        type_keyword_idx: usize,
    ) -> Option<ParsedDeclarationSymbol> {
        let name_idx = Self::next_meaningful_token_idx(item, type_keyword_idx + 1)?;
        let name =
            Self::token_word(&item[name_idx].token).and_then(Self::local_identifier_from_word)?;

        let mut depth = 0usize;
        let mut idx = name_idx + 1;
        let mut collection_keyword_idx = None;
        while idx < item.len() {
            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
                SqlToken::Word(word)
                    if depth == 0
                        && (word.eq_ignore_ascii_case("IS") || word.eq_ignore_ascii_case("AS")) =>
                {
                    let next_idx = Self::next_meaningful_token_idx(item, idx + 1)?;
                    let next_word = Self::token_word(&item[next_idx].token)?;
                    if !(next_word.eq_ignore_ascii_case("TABLE")
                        || next_word.eq_ignore_ascii_case("VARRAY")
                        || next_word.eq_ignore_ascii_case("VARYING"))
                    {
                        return None;
                    }
                    collection_keyword_idx = Some(next_idx);
                    break;
                }
                _ => {}
            }
            idx += 1;
        }

        let mut scan_idx = collection_keyword_idx?;
        if Self::token_word(&item[scan_idx].token)
            .is_some_and(|word| word.eq_ignore_ascii_case("VARYING"))
        {
            let array_idx = Self::next_meaningful_token_idx(item, scan_idx + 1)?;
            let array_word = Self::token_word(&item[array_idx].token)?;
            if !array_word.eq_ignore_ascii_case("ARRAY") {
                return None;
            }
            scan_idx = array_idx;
        }

        let mut depth = 0usize;
        let mut of_idx = None;
        let mut idx = scan_idx + 1;
        while idx < item.len() {
            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
                SqlToken::Word(word) if depth == 0 && word.eq_ignore_ascii_case("OF") => {
                    of_idx = Some(idx);
                    break;
                }
                SqlToken::Symbol(sym) if sym == ";" && depth == 0 => return None,
                _ => {}
            }
            idx += 1;
        }

        let source_idx = Self::next_meaningful_token_idx(item, of_idx?.saturating_add(1))?;
        let (member_source, source_end_idx) =
            Self::extract_declaration_type_source_name(item, source_idx)?;
        let member_source_is_rowtype =
            Self::declaration_type_source_has_percent_kind(item, source_end_idx, "ROWTYPE");
        let member_source_upper = Some(member_source.to_ascii_uppercase());
        let member_source_uppers =
            Self::rowtype_source_uppers_from_single(&member_source_upper, member_source_is_rowtype);

        Some(ParsedDeclarationSymbol {
            name,
            type_display: None,
            members: Vec::new(),
            member_entries: Vec::new(),
            member_source_upper,
            member_source_uppers,
            member_source_is_rowtype,
            member_source_is_collection_like: true,
            member_source_allows_visible_members: member_source_is_rowtype,
            suggest_name: false,
        })
    }

    fn extract_declaration_member_source(
        item: &[SqlTokenSpan],
        name_idx: usize,
    ) -> (Option<String>, bool) {
        let Some(mut type_idx) = Self::next_meaningful_token_idx(item, name_idx + 1) else {
            return (None, false);
        };
        if Self::token_word(&item[type_idx].token)
            .is_some_and(|word| word.eq_ignore_ascii_case("CONSTANT"))
        {
            let Some(next_type_idx) = Self::next_meaningful_token_idx(item, type_idx + 1) else {
                return (None, false);
            };
            type_idx = next_type_idx;
        }

        let Some((source_name, source_end_idx)) =
            Self::extract_declaration_type_source_name(item, type_idx)
        else {
            return (None, false);
        };
        let is_rowtype = Self::declaration_type_source_has_percent_kind(
            item,
            source_end_idx,
            "ROWTYPE",
        );

        (Some(source_name.to_ascii_uppercase()), is_rowtype)
    }

    fn extract_declaration_scalar_type_display(
        item: &[SqlTokenSpan],
        name_idx: usize,
    ) -> Option<String> {
        let mut type_idx = Self::next_meaningful_token_idx(item, name_idx + 1)?;
        if Self::token_word(&item[type_idx].token)
            .is_some_and(|word| word.eq_ignore_ascii_case("CONSTANT"))
        {
            type_idx = Self::next_meaningful_token_idx(item, type_idx + 1)?;
        }
        Self::scalar_type_display_at_idx(item, type_idx)
    }

    fn extract_parameter_scalar_type_display(
        item: &[SqlTokenSpan],
        name_idx: usize,
    ) -> Option<String> {
        let mut type_idx = Self::next_meaningful_token_idx(item, name_idx + 1)?;
        while let Some(word) = Self::token_word(&item[type_idx].token) {
            if !Self::is_parameter_mode_keyword(word) {
                break;
            }
            type_idx = Self::next_meaningful_token_idx(item, type_idx + 1)?;
        }
        Self::scalar_type_display_at_idx(item, type_idx)
    }

    fn scalar_type_display_at_idx(item: &[SqlTokenSpan], type_idx: usize) -> Option<String> {
        let type_name = Self::token_word(&item[type_idx].token)?;
        let type_name = sql_text::strip_identifier_quotes(type_name).to_ascii_uppercase();
        match Self::classify_type_display(&type_name) {
            PrecedingOperandType::Datetime
            | PrecedingOperandType::Character
            | PrecedingOperandType::Numeric
            | PrecedingOperandType::FloatingNumeric
            | PrecedingOperandType::Collection => Some(type_name),
            PrecedingOperandType::Other | PrecedingOperandType::Unknown => None,
        }
    }

    fn extract_declaration_type_source_name(
        item: &[SqlTokenSpan],
        start_idx: usize,
    ) -> Option<(String, usize)> {
        let first =
            Self::token_word(&item[start_idx].token).and_then(Self::local_identifier_from_word)?;
        let mut parts = vec![first];
        let mut end_idx = start_idx;

        loop {
            let Some(dot_idx) = Self::next_meaningful_token_idx(item, end_idx + 1) else {
                break;
            };
            if !Self::token_symbol_at(item, dot_idx, ".") {
                break;
            }
            let Some(part_idx) = Self::next_meaningful_token_idx(item, dot_idx + 1) else {
                break;
            };
            let Some(part) =
                Self::token_word(&item[part_idx].token).and_then(Self::local_identifier_from_word)
            else {
                break;
            };
            parts.push(part);
            end_idx = part_idx;
        }

        Some((parts.join("."), end_idx))
    }

    fn declaration_type_source_has_percent_kind(
        item: &[SqlTokenSpan],
        source_end_idx: usize,
        expected_kind: &str,
    ) -> bool {
        let Some(percent_idx) = Self::next_meaningful_token_idx(item, source_end_idx + 1) else {
            return false;
        };
        if !Self::token_symbol_at(item, percent_idx, "%") {
            return false;
        }
        let Some(kind_idx) = Self::next_meaningful_token_idx(item, percent_idx + 1) else {
            return false;
        };
        Self::token_word(&item[kind_idx].token)
            .is_some_and(|word| word.eq_ignore_ascii_case(expected_kind))
    }

    fn extract_record_type_fields(
        item: &[SqlTokenSpan],
        open_idx: usize,
        close_idx: usize,
    ) -> (Vec<String>, Vec<LocalMemberEntry>) {
        let mut names = Vec::new();
        let mut entries = Vec::new();
        let mut field_start = open_idx + 1;
        let mut depth = 0usize;
        let mut idx = field_start;

        while idx <= close_idx {
            let is_boundary = idx == close_idx
                || matches!(&item[idx].token, SqlToken::Symbol(sym) if sym == "," && depth == 0);
            if is_boundary {
                if let Some(entry) = Self::record_field_entry_from_item_range(item, field_start, idx) {
                    names.push(entry.name.clone());
                    entries.push(entry);
                }
                field_start = idx + 1;
            }

            if idx == close_idx {
                break;
            }

            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
                _ => {}
            }
            idx += 1;
        }

        (names, entries)
    }

    fn record_field_entry_from_item_range(
        item: &[SqlTokenSpan],
        start_idx: usize,
        end_idx: usize,
    ) -> Option<LocalMemberEntry> {
        let relative_name_idx = item[start_idx..end_idx]
            .iter()
            .position(|span| !matches!(span.token, SqlToken::Comment(_)))?;
        let name_idx = start_idx + relative_name_idx;
        let raw_name = Self::token_word(&item[name_idx].token)?;
        let normalized_name = Self::local_identifier_from_word(raw_name)?;
        let name = if sql_text::is_quoted_identifier(raw_name.trim()) {
            raw_name.trim().to_string()
        } else {
            normalized_name.clone()
        };

        let (type_display, member_source_upper, member_source_is_rowtype) =
            if let Some(type_idx) = Self::next_meaningful_token_idx(item, name_idx + 1)
                .filter(|idx| *idx < end_idx)
            {
                let type_display = Self::scalar_type_display_at_idx(item, type_idx);
                if let Some((source_name, source_end_idx)) =
                    Self::extract_declaration_type_source_name(item, type_idx)
                        .filter(|(_, source_end_idx)| *source_end_idx < end_idx)
                {
                    let is_rowtype =
                        Self::declaration_type_source_has_percent_kind(item, source_end_idx, "ROWTYPE");
                    (type_display, Some(source_name.to_ascii_uppercase()), is_rowtype)
                } else {
                    (type_display, None, false)
                }
            } else {
                (None, None, false)
            };
        let member_source_uppers =
            Self::rowtype_source_uppers_from_single(&member_source_upper, member_source_is_rowtype);

        Some(LocalMemberEntry {
            upper: normalized_name.to_ascii_uppercase(),
            name,
            type_display,
            member_source_upper,
            member_source_uppers,
            member_source_is_rowtype,
            member_source_is_collection_like: false,
            member_source_allows_visible_members: member_source_is_rowtype,
        })
    }

    fn extract_cursor_projection_members_and_source_from_item(
        item: &[SqlTokenSpan],
        cursor_keyword_idx: usize,
    ) -> (Vec<String>, Vec<String>) {
        let mut depth = 0usize;
        let mut idx = cursor_keyword_idx.saturating_add(1);
        while idx < item.len() {
            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => depth = depth.saturating_sub(1),
                SqlToken::Word(word)
                    if depth == 0
                        && (word.eq_ignore_ascii_case("IS") || word.eq_ignore_ascii_case("AS")) =>
                {
                    let body_tokens: Vec<SqlToken> = item[idx + 1..]
                        .iter()
                        .map(|span| span.token.clone())
                        .collect();
                    return Self::extract_select_projection_members_and_source(&body_tokens);
                }
                _ => {}
            }
            idx += 1;
        }

        (Vec::new(), Vec::new())
    }

    fn extract_mysql_declaration_symbols_from_item(
        item: &[SqlTokenSpan],
    ) -> Vec<ParsedDeclarationSymbol> {
        let Some(first_idx) = item
            .iter()
            .position(|span| !matches!(span.token, SqlToken::Comment(_)))
        else {
            return Vec::new();
        };

        let Some(first_word) = Self::token_word(&item[first_idx].token) else {
            return Vec::new();
        };
        if !first_word.eq_ignore_ascii_case("DECLARE") {
            return Vec::new();
        }

        let meaningful_words: Vec<&str> = item[first_idx + 1..]
            .iter()
            .filter_map(|span| Self::token_word(&span.token))
            .collect();
        let Some(first_after_declare) = meaningful_words.first().copied() else {
            return Vec::new();
        };

        if matches!(
            first_after_declare.to_ascii_uppercase().as_str(),
            "CONTINUE" | "EXIT" | "UNDO" | "HANDLER"
        ) {
            return Vec::new();
        }

        let mut names = Vec::new();
        let mut idx = first_idx.saturating_add(1);
        let mut expecting_name = true;
        let mut type_display = None;

        while idx < item.len() {
            match &item[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "," && !names.is_empty() => {
                    expecting_name = true;
                }
                SqlToken::Word(word) if expecting_name => {
                    let upper = word.to_ascii_uppercase();
                    if upper == "DECLARE" {
                        idx += 1;
                        continue;
                    }
                    if let Some(name) = Self::local_identifier_suggestion_from_word(word) {
                        names.push(name);
                        expecting_name = false;
                    } else {
                        break;
                    }
                }
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    if upper == "CURSOR" || upper == "CONDITION" {
                        break;
                    }
                    type_display = Self::scalar_type_display_at_idx(item, idx);
                    break;
                }
                SqlToken::Symbol(_) => {
                    break;
                }
                _ => {}
            }
            idx += 1;
        }

        names
            .into_iter()
            .map(|name| ParsedDeclarationSymbol {
                name,
                type_display: type_display.clone(),
                members: Vec::new(),
                member_entries: Vec::new(),
                member_source_upper: None,
                member_source_uppers: Vec::new(),
                member_source_is_rowtype: false,
                member_source_is_collection_like: false,
                member_source_allows_visible_members: false,
                suggest_name: true,
            })
            .collect()
    }

    fn declaration_item_declared_at(item: &[SqlTokenSpan]) -> usize {
        let mut last = 0usize;
        for span in item {
            match &span.token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == ";" => {
                    return span.start;
                }
                _ => last = span.end,
            }
        }
        last
    }

    fn find_statement_item_end(
        token_spans: &[SqlTokenSpan],
        item_start: usize,
        limit: usize,
    ) -> usize {
        let mut item_end = item_start;
        let mut paren_depth = 0usize;
        while item_end < limit {
            match &token_spans[item_end].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    paren_depth = paren_depth.saturating_add(1);
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                SqlToken::Symbol(sym) if sym == ";" && paren_depth == 0 => {
                    item_end += 1;
                    break;
                }
                _ => {}
            }
            item_end += 1;
        }
        item_end
    }

    fn parse_for_loop_record(tokens: &[SqlTokenSpan], idx: usize) -> Option<ParsedForLoopRecord> {
        let name_idx = Self::next_meaningful_token_idx(tokens, idx + 1)?;
        let name =
            Self::token_word(&tokens[name_idx].token)
                .and_then(Self::local_identifier_suggestion_from_word)?;
        let in_idx = Self::next_meaningful_token_idx(tokens, name_idx + 1)?;
        let in_word = Self::token_word(&tokens[in_idx].token)?;
        if !in_word.eq_ignore_ascii_case("IN") {
            return None;
        }
        let Some(open_idx) = Self::next_meaningful_token_idx(tokens, in_idx + 1) else {
            return Some(ParsedForLoopRecord {
                name,
                members: Vec::new(),
                member_source_upper: None,
                member_source_uppers: Vec::new(),
                member_source_is_rowtype: false,
            });
        };
        if Self::token_symbol_at(tokens, open_idx, "(") {
            let (members, member_source_uppers) =
                Self::extract_for_loop_select_projection_members_and_source(tokens, open_idx);
            let member_source_upper = member_source_uppers.first().cloned();
            return Some(ParsedForLoopRecord {
                name,
                members,
                member_source_upper,
                member_source_uppers,
                member_source_is_rowtype: true,
            });
        }

        let member_source_upper = Self::token_word(&tokens[open_idx].token)
            .and_then(Self::local_identifier_from_word)
            .map(|source| source.to_ascii_uppercase());
        Some(ParsedForLoopRecord {
            name,
            members: Vec::new(),
            member_source_upper,
            member_source_uppers: Vec::new(),
            member_source_is_rowtype: false,
        })
    }

    fn resolve_deferred_local_record_members(
        scopes: &[LocalScope],
        symbols: &mut [LocalSymbolEntry],
    ) {
        let scope_rank_by_scope = Self::local_scope_rank_maps(scopes);

        for _ in 0..symbols.len() {
            let mut candidates_by_upper = HashMap::<String, Vec<usize>>::new();
            for (idx, symbol) in symbols.iter().enumerate() {
                if symbol.members.is_empty() && !symbol.member_source_is_rowtype {
                    continue;
                }
                candidates_by_upper
                    .entry(symbol.upper.clone())
                    .or_default()
                    .push(idx);
            }

            let mut resolved_members = vec![None; symbols.len()];
            let mut resolved_member_entries = vec![None; symbols.len()];
            let mut resolved_rowtype_metadata = vec![None; symbols.len()];
            let mut resolved_rowtype_sources = vec![None; symbols.len()];
            let mut resolved_collection_flags = vec![None; symbols.len()];
            let mut resolved_visible_member_flags = vec![None; symbols.len()];

            for (idx, symbol) in symbols.iter().enumerate() {
                let Some(source_upper) = symbol.member_source_upper.as_deref() else {
                    continue;
                };
                if !symbol.members.is_empty() {
                    continue;
                }

                let Some(scope_ranks) = scope_rank_by_scope.get(symbol.scope_id) else {
                    continue;
                };
                let mut best: Option<(usize, usize, usize)> = None;
                let Some(candidate_indices) = candidates_by_upper.get(source_upper) else {
                    continue;
                };
                for &candidate_idx in candidate_indices {
                    let candidate = &symbols[candidate_idx];
                    if candidate.declared_at > symbol.declared_at {
                        continue;
                    }
                    if candidate.suggest_name && !symbol.member_source_allows_visible_members {
                        continue;
                    }
                    let Some(scope_rank) = scope_ranks.get(&candidate.scope_id).copied() else {
                        continue;
                    };
                    if best.is_none_or(|(best_rank, best_declared_at, _)| {
                        scope_rank < best_rank
                            || (scope_rank == best_rank && candidate.declared_at >= best_declared_at)
                    }) {
                        best = Some((scope_rank, candidate.declared_at, candidate_idx));
                    }
                }

                if let Some((_, _, candidate_idx)) = best {
                    let candidate = &symbols[candidate_idx];
                    if !candidate.members.is_empty() {
                        resolved_members[idx] = Some(candidate.members.clone());
                        resolved_member_entries[idx] = Some(candidate.member_entries.clone());
                        resolved_rowtype_metadata[idx] = Some((
                            candidate.member_source_upper.clone(),
                            candidate.member_source_is_rowtype
                                && candidate.member_source_upper.is_some(),
                        ));
                        resolved_rowtype_sources[idx] = Some(candidate.member_source_uppers.clone());
                        resolved_collection_flags[idx] = Some(
                            symbol.member_source_is_collection_like
                                || candidate.member_source_is_collection_like,
                        );
                        resolved_visible_member_flags[idx] = Some(
                            symbol.member_source_allows_visible_members
                                || candidate.member_source_allows_visible_members,
                        );
                    } else if candidate.member_source_is_rowtype {
                        resolved_rowtype_metadata[idx] = Some((
                            candidate.member_source_upper.clone(),
                            candidate.member_source_upper.is_some(),
                        ));
                        resolved_rowtype_sources[idx] = Some(candidate.member_source_uppers.clone());
                        resolved_collection_flags[idx] = Some(
                            symbol.member_source_is_collection_like
                                || candidate.member_source_is_collection_like,
                        );
                        resolved_visible_member_flags[idx] = Some(
                            symbol.member_source_allows_visible_members
                                || candidate.member_source_allows_visible_members,
                        );
                    }
                }
            }

            let mut changed = false;
            for (
                (((((symbol, members), member_entries), rowtype_metadata), rowtype_sources), collection_like),
                allows_visible_members,
            ) in symbols
                .iter_mut()
                .zip(resolved_members)
                .zip(resolved_member_entries)
                .zip(resolved_rowtype_metadata)
                .zip(resolved_rowtype_sources)
                .zip(resolved_collection_flags)
                .zip(resolved_visible_member_flags)
            {
                if let Some(members) = members {
                    symbol.members = members;
                    changed = true;
                }
                if let Some(member_entries) = member_entries {
                    symbol.member_entries = member_entries;
                    changed = true;
                }
                if let Some((rowtype_source, is_rowtype)) = rowtype_metadata {
                    if symbol.member_source_upper != rowtype_source
                        || symbol.member_source_is_rowtype != is_rowtype
                    {
                        symbol.member_source_upper = rowtype_source;
                        symbol.member_source_is_rowtype = is_rowtype;
                        changed = true;
                    }
                }
                if let Some(rowtype_sources) = rowtype_sources {
                    if symbol.member_source_uppers != rowtype_sources {
                        symbol.member_source_uppers = rowtype_sources;
                        changed = true;
                    }
                }
                if let Some(collection_like) = collection_like {
                    if symbol.member_source_is_collection_like != collection_like {
                        symbol.member_source_is_collection_like = collection_like;
                        changed = true;
                    }
                }
                if let Some(allows_visible_members) = allows_visible_members {
                    if symbol.member_source_allows_visible_members != allows_visible_members {
                        symbol.member_source_allows_visible_members = allows_visible_members;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn local_scope_rank_maps(scopes: &[LocalScope]) -> Vec<HashMap<usize, usize>> {
        (0..scopes.len())
            .map(|scope_id| {
                Self::local_scope_chain(scopes, scope_id)
                    .into_iter()
                    .enumerate()
                    .map(|(rank, visible_scope_id)| (visible_scope_id, rank))
                    .collect()
            })
            .collect()
    }

    fn extract_for_loop_select_projection_members_and_source(
        tokens: &[SqlTokenSpan],
        open_idx: usize,
    ) -> (Vec<String>, Vec<String>) {
        let Some(close_idx) = Self::matching_local_paren_idx(tokens, open_idx) else {
            return (Vec::new(), Vec::new());
        };
        if close_idx <= open_idx + 1 {
            return (Vec::new(), Vec::new());
        }

        let body_tokens: Vec<SqlToken> = tokens[open_idx + 1..close_idx]
            .iter()
            .map(|span| span.token.clone())
            .collect();
        Self::extract_select_projection_members_and_source(&body_tokens)
    }

    fn extract_select_projection_members_and_source(
        body_tokens: &[SqlToken],
    ) -> (Vec<String>, Vec<String>) {
        let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
        Self::dedup_column_names_case_insensitive(&mut columns);
        if let Some(transformed_columns) =
            Self::oracle_pivot_unpivot_replacement_projection(body_tokens, &columns)
        {
            return (transformed_columns, Vec::new());
        }

        let tables_in_scope = intellisense_context::collect_tables_in_statement(body_tokens);
        let mut wildcard_tables =
            intellisense_context::extract_select_list_wildcard_scopes(body_tokens, &tables_in_scope);
        let virtual_wildcard_columns =
            Self::extract_virtual_wildcard_projection_members(body_tokens, &mut wildcard_tables);
        columns.extend(virtual_wildcard_columns);
        Self::dedup_column_names_case_insensitive(&mut columns);
        for table in &mut wildcard_tables {
            *table = table.to_ascii_uppercase();
        }
        Self::dedup_column_names_case_insensitive(&mut wildcard_tables);
        (columns, wildcard_tables)
    }

    fn oracle_pivot_unpivot_replacement_projection(
        body_tokens: &[SqlToken],
        columns: &[String],
    ) -> Option<Vec<String>> {
        let mut projection =
            intellisense_context::extract_oracle_pivot_unpivot_projection_columns(body_tokens);
        if projection.is_empty() {
            return None;
        }

        if columns.is_empty() {
            Self::dedup_column_names_case_insensitive(&mut projection);
            return Some(projection);
        }

        let source_columns =
            intellisense_context::extract_oracle_pivot_unpivot_source_projection_columns(
                body_tokens,
            );
        if Self::local_column_sets_match_case_insensitive(columns, &source_columns) {
            Self::dedup_column_names_case_insensitive(&mut projection);
            return Some(projection);
        }

        None
    }

    fn extract_virtual_wildcard_projection_members(
        body_tokens: &[SqlToken],
        wildcard_tables: &mut Vec<String>,
    ) -> Vec<String> {
        if wildcard_tables.is_empty() {
            return Vec::new();
        }

        let ctx = intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
        let mut virtual_members_by_name = HashMap::<String, VirtualProjectionMembers>::new();
        for cte in &ctx.ctes {
            let mut columns = cte.explicit_columns.clone();
            let mut rowtype_sources = Vec::new();
            if columns.is_empty() && !cte.body_range.is_empty() {
                let members = Self::extract_known_virtual_projection_members(
                    intellisense_context::token_range_slice(
                        ctx.statement_tokens.as_ref(),
                        cte.body_range,
                    ),
                );
                columns = members.columns;
                rowtype_sources = members.rowtype_sources;
            }
            Self::dedup_column_names_case_insensitive(&mut columns);
            for source in &mut rowtype_sources {
                *source = source.to_ascii_uppercase();
            }
            Self::dedup_column_names_case_insensitive(&mut rowtype_sources);
            if !columns.is_empty() || !rowtype_sources.is_empty() {
                virtual_members_by_name.insert(
                    cte.name.to_ascii_uppercase(),
                    VirtualProjectionMembers {
                        columns,
                        rowtype_sources,
                    },
                );
            }
        }
        for subquery in &ctx.subqueries {
            let mut members = if subquery.explicit_columns.is_empty() {
                Self::extract_known_virtual_projection_members(intellisense_context::token_range_slice(
                    ctx.statement_tokens.as_ref(),
                    subquery.body_range,
                ))
            } else {
                VirtualProjectionMembers {
                    columns: subquery.explicit_columns.clone(),
                    rowtype_sources: Vec::new(),
                }
            };
            for source in &mut members.rowtype_sources {
                *source = source.to_ascii_uppercase();
            }
            Self::dedup_column_names_case_insensitive(&mut members.columns);
            Self::dedup_column_names_case_insensitive(&mut members.rowtype_sources);
            if !members.columns.is_empty() || !members.rowtype_sources.is_empty() {
                virtual_members_by_name.insert(subquery.alias.to_ascii_uppercase(), members);
            }
        }

        if virtual_members_by_name.is_empty() {
            Self::normalize_wildcard_scopes_to_rowtype_sources(
                &ctx.tables_in_scope,
                wildcard_tables,
            );
            return Vec::new();
        }

        let mut columns = Vec::new();
        let mut remaining_wildcard_tables = Vec::new();
        for table in wildcard_tables.drain(..) {
            let key = ctx
                .tables_in_scope
                .iter()
                .find_map(|table_ref| {
                    if table_ref
                        .alias
                        .as_deref()
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(&table))
                        && virtual_members_by_name
                            .contains_key(&table_ref.name.to_ascii_uppercase())
                    {
                        return Some(table_ref.name.to_ascii_uppercase());
                    }
                    let alias = table_ref.alias.as_deref()?;
                    (table_ref.name.eq_ignore_ascii_case(&table)
                        && virtual_members_by_name
                            .contains_key(&alias.to_ascii_uppercase()))
                    .then(|| alias.to_ascii_uppercase())
                })
                .unwrap_or_else(|| table.to_ascii_uppercase());
            if virtual_members_by_name.contains_key(&key) {
                let mut visiting = HashSet::new();
                Self::append_virtual_projection_members_for_key(
                    &key,
                    &virtual_members_by_name,
                    &mut columns,
                    &mut remaining_wildcard_tables,
                    &mut visiting,
                );
            } else {
                let source_name = ctx
                    .tables_in_scope
                    .iter()
                    .find_map(|table_ref| {
                        table_ref
                            .alias
                            .as_deref()
                            .filter(|alias| alias.eq_ignore_ascii_case(&table))
                            .map(|_| table_ref.name.clone())
                    })
                    .unwrap_or(table);
                remaining_wildcard_tables.push(source_name);
            }
        }
        *wildcard_tables = remaining_wildcard_tables;
        Self::dedup_column_names_case_insensitive(&mut columns);
        columns
    }

    fn normalize_wildcard_scopes_to_rowtype_sources(
        tables_in_scope: &[intellisense_context::ScopedTableRef],
        wildcard_tables: &mut [String],
    ) {
        for table in wildcard_tables {
            if let Some(source_name) = tables_in_scope.iter().find_map(|table_ref| {
                table_ref
                    .alias
                    .as_deref()
                    .filter(|alias| alias.eq_ignore_ascii_case(table))
                    .map(|_| table_ref.name.clone())
            }) {
                *table = source_name;
            }
        }
    }

    fn append_virtual_projection_members_for_key(
        key: &str,
        virtual_members_by_name: &HashMap<String, VirtualProjectionMembers>,
        columns: &mut Vec<String>,
        rowtype_sources: &mut Vec<String>,
        visiting: &mut HashSet<String>,
    ) {
        let key_upper = key.to_ascii_uppercase();
        if !visiting.insert(key_upper.clone()) {
            return;
        }

        if let Some(virtual_members) = virtual_members_by_name.get(&key_upper) {
            columns.extend(virtual_members.columns.iter().cloned());
            for source in &virtual_members.rowtype_sources {
                let source_upper = source.to_ascii_uppercase();
                if virtual_members_by_name.contains_key(&source_upper) {
                    Self::append_virtual_projection_members_for_key(
                        &source_upper,
                        virtual_members_by_name,
                        columns,
                        rowtype_sources,
                        visiting,
                    );
                } else {
                    rowtype_sources.push(source.clone());
                }
            }
        }

        visiting.remove(&key_upper);
    }

    fn extract_known_virtual_projection_members(body_tokens: &[SqlToken]) -> VirtualProjectionMembers {
        let (mut columns, mut rowtype_sources) =
            Self::extract_select_projection_members_and_source(body_tokens);
        if columns.is_empty() {
            columns = intellisense_context::extract_table_function_columns(body_tokens);
        }
        if columns.is_empty() {
            if let Some(transformed_columns) =
                Self::oracle_pivot_unpivot_replacement_projection(body_tokens, &columns)
            {
                columns = transformed_columns;
                rowtype_sources.clear();
            }
        }
        columns.extend(intellisense_context::extract_oracle_model_generated_columns(
            body_tokens,
        ));
        columns.extend(intellisense_context::extract_match_recognize_generated_columns(
            body_tokens,
        ));
        Self::dedup_column_names_case_insensitive(&mut columns);
        for source in &mut rowtype_sources {
            *source = source.to_ascii_uppercase();
        }
        Self::dedup_column_names_case_insensitive(&mut rowtype_sources);
        VirtualProjectionMembers {
            columns,
            rowtype_sources,
        }
    }

    fn matching_local_paren_idx(tokens: &[SqlTokenSpan], open_idx: usize) -> Option<usize> {
        if !Self::token_symbol_at(tokens, open_idx, "(") {
            return None;
        }

        let mut depth = 1usize;
        let mut idx = open_idx.saturating_add(1);
        while idx < tokens.len() {
            match &tokens[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => depth = depth.saturating_add(1),
                SqlToken::Symbol(sym) if sym == ")" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(idx);
                    }
                }
                _ => {}
            }
            idx += 1;
        }

        None
    }

    fn parse_package_body_header(
        tokens: &[SqlTokenSpan],
        idx: usize,
    ) -> Option<ParsedPackageBodyHeader> {
        let body_idx = Self::next_meaningful_token_idx(tokens, idx + 1)?;
        let body_word = Self::token_word(&tokens[body_idx].token)?;
        if !body_word.eq_ignore_ascii_case("BODY") {
            return None;
        }

        let mut scan_idx = body_idx + 1;
        while scan_idx < tokens.len() {
            let token = &tokens[scan_idx];
            match &token.token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == ";" => return None,
                SqlToken::Word(word)
                    if word.eq_ignore_ascii_case("AS") || word.eq_ignore_ascii_case("IS") =>
                {
                    return Some(ParsedPackageBodyHeader {
                        body_keyword_idx: scan_idx,
                        decl_start_idx: scan_idx.saturating_add(1),
                    });
                }
                _ => {}
            }
            scan_idx += 1;
        }

        None
    }

    fn parse_routine_header(tokens: &[SqlTokenSpan], idx: usize) -> Option<ParsedRoutineHeader> {
        let name_idx = Self::next_meaningful_token_idx(tokens, idx + 1)?;
        let name_word = Self::token_word(&tokens[name_idx].token)?;
        let _ = Self::local_identifier_from_word(name_word)?;

        let mut scan_idx = Self::next_meaningful_token_idx(tokens, name_idx + 1).unwrap_or(tokens.len());
        let mut parameters = Vec::new();
        let mut return_type_display = None;
        let mut saw_mysql_returns = false;
        if scan_idx < tokens.len() && Self::token_symbol_at(tokens, scan_idx, "(") {
            let (close_idx, parsed_parameters) = Self::extract_parameter_symbols(tokens, scan_idx)?;
            parameters = parsed_parameters;
            scan_idx =
                Self::next_meaningful_token_idx(tokens, close_idx.saturating_add(1)).unwrap_or(tokens.len());
        }

        let mut paren_depth = 0usize;
        while scan_idx < tokens.len() {
            match &tokens[scan_idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    paren_depth = paren_depth.saturating_add(1);
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                SqlToken::Symbol(sym) if sym == ";" && paren_depth == 0 => {
                    return None;
                }
                SqlToken::Word(word)
                    if paren_depth == 0
                        && (word.eq_ignore_ascii_case("AS") || word.eq_ignore_ascii_case("IS")) =>
                {
                    if Self::routine_header_is_external_call_spec(tokens, scan_idx) {
                        return None;
                    }
                    return Some(ParsedRoutineHeader {
                        body_keyword_idx: scan_idx,
                        decl_start_idx: scan_idx.saturating_add(1),
                        parameters,
                        return_type_display,
                        body_starts_immediately: false,
                    });
                }
                SqlToken::Word(word) if paren_depth == 0 && word.eq_ignore_ascii_case("RETURNS") => {
                    saw_mysql_returns = true;
                    return_type_display = Self::next_meaningful_token_idx(tokens, scan_idx + 1)
                        .and_then(|type_idx| Self::scalar_type_display_at_idx(tokens, type_idx));
                }
                SqlToken::Word(word)
                    if paren_depth == 0
                        && !saw_mysql_returns
                        && word.eq_ignore_ascii_case("RETURN") =>
                {
                    return_type_display = Self::next_meaningful_token_idx(tokens, scan_idx + 1)
                        .and_then(|type_idx| Self::scalar_type_display_at_idx(tokens, type_idx));
                }
                SqlToken::Word(word) if paren_depth == 0 && word.eq_ignore_ascii_case("BEGIN") => {
                    return Some(ParsedRoutineHeader {
                        body_keyword_idx: scan_idx,
                        decl_start_idx: scan_idx,
                        parameters,
                        return_type_display,
                        body_starts_immediately: true,
                    });
                }
                SqlToken::Word(word)
                    if paren_depth == 0
                        && saw_mysql_returns
                        && word.eq_ignore_ascii_case("RETURN") =>
                {
                    return Some(ParsedRoutineHeader {
                        body_keyword_idx: scan_idx,
                        decl_start_idx: scan_idx,
                        parameters,
                        return_type_display,
                        body_starts_immediately: true,
                    });
                }
                _ => {}
            }
            scan_idx += 1;
        }

        None
    }

    fn routine_header_is_external_call_spec(
        tokens: &[SqlTokenSpan],
        body_keyword_idx: usize,
    ) -> bool {
        let mut idx = body_keyword_idx.saturating_add(1);
        let mut paren_depth = 0usize;
        let mut saw_external_clause = false;

        while idx < tokens.len() {
            match &tokens[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    paren_depth = paren_depth.saturating_add(1);
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                SqlToken::Symbol(sym) if sym == ";" && paren_depth == 0 => {
                    return saw_external_clause;
                }
                SqlToken::Word(word) if paren_depth == 0 => {
                    if word.eq_ignore_ascii_case("BEGIN") || word.eq_ignore_ascii_case("DECLARE") {
                        return false;
                    }
                    if matches!(
                        word.to_ascii_uppercase().as_str(),
                        "LANGUAGE" | "EXTERNAL" | "LIBRARY" | "PARAMETERS" | "NAME"
                    ) {
                        saw_external_clause = true;
                    }
                }
                _ => {}
            }
            idx += 1;
        }

        false
    }

    fn extract_parameter_symbols(
        tokens: &[SqlTokenSpan],
        open_idx: usize,
    ) -> Option<(usize, Vec<ParsedDeclarationSymbol>)> {
        if !Self::token_symbol_at(tokens, open_idx, "(") {
            return None;
        }

        let mut idx = open_idx + 1;
        let mut depth = 1usize;
        let mut item_start = idx;
        let mut parameters = Vec::new();

        while idx < tokens.len() {
            match &tokens[idx].token {
                SqlToken::Comment(_) => {}
                SqlToken::Symbol(sym) if sym == "(" => {
                    depth = depth.saturating_add(1);
                }
                SqlToken::Symbol(sym) if sym == ")" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if item_start < idx {
                            if let Some(parameter) =
                                Self::extract_parameter_symbol_from_item(&tokens[item_start..idx])
                            {
                                parameters.push(parameter);
                            }
                        }
                        return Some((idx, parameters));
                    }
                }
                SqlToken::Symbol(sym) if sym == "," && depth == 1 => {
                    if item_start < idx {
                        if let Some(parameter) =
                            Self::extract_parameter_symbol_from_item(&tokens[item_start..idx])
                        {
                            parameters.push(parameter);
                        }
                    }
                    item_start = idx.saturating_add(1);
                }
                _ => {}
            }
            idx += 1;
        }

        None
    }

    fn extract_parameter_symbol_from_item(item: &[SqlTokenSpan]) -> Option<ParsedDeclarationSymbol> {
        let name_idx = Self::parameter_name_idx_from_item(item)?;
        let name =
            Self::token_word(&item[name_idx].token)
                .and_then(Self::local_identifier_suggestion_from_word)?;
        let (member_source_upper, member_source_is_rowtype) =
            Self::extract_parameter_member_source(item, name_idx);
        let type_display = Self::extract_parameter_scalar_type_display(item, name_idx);
        let member_source_uppers =
            Self::rowtype_source_uppers_from_single(&member_source_upper, member_source_is_rowtype);

        Some(ParsedDeclarationSymbol {
            name,
            type_display,
            members: Vec::new(),
            member_entries: Vec::new(),
            member_source_upper,
            member_source_uppers,
            member_source_is_rowtype,
            member_source_is_collection_like: false,
            member_source_allows_visible_members: member_source_is_rowtype,
            suggest_name: true,
        })
    }

    fn parameter_name_idx_from_item(item: &[SqlTokenSpan]) -> Option<usize> {
        let mut idx = item
            .iter()
            .position(|span| !matches!(span.token, SqlToken::Comment(_)))?;

        while idx < item.len() {
            let word = Self::token_word(&item[idx].token)?;
            if !Self::is_parameter_mode_keyword(word) {
                break;
            }
            idx = Self::next_meaningful_token_idx(item, idx + 1)?;
        }

        Self::token_word(&item[idx].token)
            .and_then(Self::local_identifier_from_word)
            .map(|_| idx)
    }

    fn extract_parameter_member_source(
        item: &[SqlTokenSpan],
        name_idx: usize,
    ) -> (Option<String>, bool) {
        let Some(mut type_idx) = Self::next_meaningful_token_idx(item, name_idx + 1) else {
            return (None, false);
        };

        while let Some(word) = Self::token_word(&item[type_idx].token) {
            if !Self::is_parameter_mode_keyword(word) {
                break;
            }
            let Some(next_idx) = Self::next_meaningful_token_idx(item, type_idx + 1) else {
                return (None, false);
            };
            type_idx = next_idx;
        }

        let Some((source_name, source_end_idx)) =
            Self::extract_declaration_type_source_name(item, type_idx)
        else {
            return (None, false);
        };
        let is_rowtype = Self::declaration_type_source_has_percent_kind(
            item,
            source_end_idx,
            "ROWTYPE",
        );

        (Some(source_name.to_ascii_uppercase()), is_rowtype)
    }

    fn is_parameter_mode_keyword(word: &str) -> bool {
        matches!(
            word.to_ascii_uppercase().as_str(),
            "IN" | "OUT" | "INOUT" | "NOCOPY"
        )
    }

    fn collect_text_var_bind_names_before_statement(
        full_text: &str,
        statement_start: usize,
    ) -> Vec<String> {
        let statement_start =
            Self::clamp_to_char_boundary_local(full_text, statement_start.min(full_text.len()));
        let tentative_start = statement_start.saturating_sub(INTELLISENSE_TEXT_BIND_SCAN_WINDOW);
        let scan_start = full_text
            .get(..tentative_start)
            .and_then(|prefix| prefix.rfind('\n').map(|idx| idx + 1))
            .unwrap_or(0);
        let scan_start = Self::clamp_to_char_boundary_local(full_text, scan_start);
        let prefix = full_text.get(scan_start..statement_start).unwrap_or("");

        let mut names = Vec::new();
        let mut seen = HashSet::new();
        for line in prefix.lines() {
            let Some(command) = QueryExecutor::parse_tool_command(line.trim()) else {
                continue;
            };
            let ToolCommand::Var { name, .. } = command else {
                continue;
            };
            let normalized = SessionState::normalize_name(&name);
            if normalized.is_empty() {
                continue;
            }
            let upper = normalized.to_ascii_uppercase();
            if seen.insert(upper) {
                names.push(normalized);
            }
        }
        names
    }

    fn current_local_parent_scope_id(block_stack: &[LocalBlockFrame]) -> usize {
        block_stack
            .iter()
            .rev()
            .find_map(|frame| frame.scope_id)
            .unwrap_or(0)
    }

    fn scope_uses_mysql_declare_statements(
        scopes: &[LocalScopeBuilder],
        mut scope_id: usize,
    ) -> bool {
        loop {
            if scopes
                .get(scope_id)
                .is_some_and(|scope| scope.mysql_declare_statements)
            {
                return true;
            }
            let Some(parent) = scopes.get(scope_id).and_then(|scope| scope.scope.parent) else {
                return false;
            };
            scope_id = parent;
        }
    }

    fn current_scope_uses_mysql_declare_statements(
        scopes: &[LocalScopeBuilder],
        block_stack: &[LocalBlockFrame],
    ) -> bool {
        let scope_id = Self::current_local_parent_scope_id(block_stack);
        Self::scope_uses_mysql_declare_statements(scopes, scope_id)
    }

    fn pop_local_block_kind(
        block_stack: &mut Vec<LocalBlockFrame>,
        scopes: &mut [LocalScopeBuilder],
        kind: LocalBlockKind,
        end_byte: usize,
        end_token_idx: usize,
    ) {
        if let Some(pos) = block_stack.iter().rposition(|frame| frame.kind == kind) {
            let frame = block_stack.remove(pos);
            Self::close_local_scope_frame(frame, scopes, end_byte, end_token_idx);
        }
    }

    fn pop_local_block(
        block_stack: &mut Vec<LocalBlockFrame>,
        scopes: &mut [LocalScopeBuilder],
        end_byte: usize,
        end_token_idx: usize,
    ) {
        if let Some(frame) = block_stack.pop() {
            Self::close_local_scope_frame(frame, scopes, end_byte, end_token_idx);
        }
    }

    fn close_local_scope_frame(
        frame: LocalBlockFrame,
        scopes: &mut [LocalScopeBuilder],
        end_byte: usize,
        end_token_idx: usize,
    ) {
        let Some(scope_id) = frame.scope_id else {
            return;
        };
        scopes[scope_id].scope.end = end_byte;
        scopes[scope_id].token_end_idx = end_token_idx;
        if scopes[scope_id].decl_end_idx.is_none() && frame.awaiting_body_begin {
            scopes[scope_id].decl_end_idx = Some(end_token_idx);
        }
    }

    fn push_local_symbol_with_metadata_and_sources(
        symbols: &mut Vec<LocalSymbolEntry>,
        seen_symbol_keys: &mut HashSet<(usize, usize, String)>,
        scope_id: usize,
        name: String,
        declared_at: usize,
        type_display: Option<String>,
        members: Vec<String>,
        member_entries: Vec<LocalMemberEntry>,
        member_source_upper: Option<String>,
        member_source_uppers: Vec<String>,
        member_source_is_rowtype: bool,
        member_source_is_collection_like: bool,
        member_source_allows_visible_members: bool,
        suggest_name: bool,
    ) {
        let upper = Self::local_identifier_lookup_upper(&name);
        if !seen_symbol_keys.insert((scope_id, declared_at, upper.clone())) {
            return;
        }
        symbols.push(LocalSymbolEntry {
            scope_id,
            upper,
            name,
            declared_at,
            type_display,
            members,
            member_entries,
            member_source_upper,
            member_source_uppers,
            member_source_is_rowtype,
            member_source_is_collection_like,
            member_source_allows_visible_members,
            suggest_name,
        });
    }

    fn rowtype_source_uppers_from_single(
        member_source_upper: &Option<String>,
        member_source_is_rowtype: bool,
    ) -> Vec<String> {
        if member_source_is_rowtype {
            member_source_upper.iter().cloned().collect()
        } else {
            Vec::new()
        }
    }

    fn next_meaningful_token_idx(tokens: &[SqlTokenSpan], start_idx: usize) -> Option<usize> {
        let mut idx = start_idx;
        while idx < tokens.len() {
            if !matches!(tokens[idx].token, SqlToken::Comment(_)) {
                return Some(idx);
            }
            idx += 1;
        }
        None
    }

    fn token_word(token: &SqlToken) -> Option<&str> {
        match token {
            SqlToken::Word(word) => Some(word.as_str()),
            _ => None,
        }
    }

    fn token_symbol_at(tokens: &[SqlTokenSpan], idx: usize, symbol: &str) -> bool {
        tokens
            .get(idx)
            .is_some_and(|span| Self::token_symbol_is(&span.token, symbol))
    }

    fn token_symbol_is(token: &SqlToken, symbol: &str) -> bool {
        matches!(token, SqlToken::Symbol(sym) if sym == symbol)
    }

    fn local_identifier_from_word(word: &str) -> Option<String> {
        let trimmed = word.trim();
        if trimmed.starts_with("<<") && trimmed.ends_with(">>") {
            return None;
        }

        let is_quoted = sql_text::is_quoted_identifier(trimmed);
        let normalized = sql_text::strip_identifier_quotes(trimmed);
        if normalized.is_empty() {
            return None;
        }

        let mut chars = normalized.chars();
        let first = chars.next()?;
        if !sql_text::is_identifier_start_char(first) {
            return None;
        }

        if !is_quoted && sql_text::is_oracle_sql_keyword(&normalized.to_ascii_uppercase()) {
            return None;
        }

        Some(normalized)
    }

    fn local_identifier_suggestion_from_word(word: &str) -> Option<String> {
        let normalized = Self::local_identifier_from_word(word)?;
        let trimmed = word.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            Some(trimmed.to_string())
        } else {
            Some(normalized)
        }
    }

    fn local_identifier_lookup_upper(identifier: &str) -> String {
        let trimmed = identifier.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            sql_text::strip_identifier_quotes(trimmed).to_ascii_uppercase()
        } else if matches!(trimmed.chars().next(), Some('"') | Some('`') | Some('[')) {
            trimmed[1..].to_ascii_uppercase()
        } else {
            identifier.to_ascii_uppercase()
        }
    }

    fn local_column_sets_match_case_insensitive(left: &[String], right: &[String]) -> bool {
        if left.len() != right.len() {
            return false;
        }

        let mut left_keys: Vec<String> = left
            .iter()
            .map(|column| Self::local_identifier_lookup_upper(column))
            .collect();
        let mut right_keys: Vec<String> = right
            .iter()
            .map(|column| Self::local_identifier_lookup_upper(column))
            .collect();
        left_keys.sort_unstable();
        right_keys.sort_unstable();
        left_keys == right_keys
    }

    #[cfg(test)]
    fn build_routine_symbol_cache_bundle_for_test(
        full_text: &str,
        cursor_pos: usize,
    ) -> (RoutineSymbolCacheEntry, ExpandedStatementWindow) {
        let expanded = Self::expanded_statement_window_in_text(full_text, cursor_pos);
        let text_bind_names =
            Self::collect_text_var_bind_names_before_statement(full_text, expanded.statement_start);
        let routine_cache = Self::build_routine_symbol_cache_entry(0, &expanded, text_bind_names);
        (routine_cache, expanded)
    }

    #[cfg(test)]
    fn collect_local_symbol_suggestions_for_test(
        script_with_cursor: &str,
        session_bind_names: &[&str],
    ) -> Vec<String> {
        Self::collect_local_symbol_suggestions_with_prefix_for_test(
            script_with_cursor,
            "",
            session_bind_names,
        )
    }

    #[cfg(test)]
    fn collect_local_symbol_suggestions_with_prefix_for_test(
        script_with_cursor: &str,
        prefix: &str,
        session_bind_names: &[&str],
    ) -> Vec<String> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let Some(cursor) = script_with_cursor.find(CURSOR_MARKER) else {
            return Vec::new();
        };
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let session_bind_names: Vec<String> = session_bind_names
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        Self::collect_local_symbol_suggestions(
            prefix,
            expanded.cursor_in_statement,
            &analysis,
            &session_bind_names,
        )
    }

    #[cfg(test)]
    fn collect_local_record_member_suggestions_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);

        Self::collect_local_record_member_suggestions(
            qualifier,
            prefix,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        )
    }

    #[cfg(test)]
    fn collect_local_record_member_suggestions_with_expected_type_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
        expected_type: ExpectedOperandTypes,
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);
        let mut suggestions = Self::collect_local_record_member_suggestions(
            qualifier,
            prefix,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        )?;

        Self::filter_local_record_member_suggestions_by_expected_operand_type(
            &mut suggestions,
            qualifier,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
            expected_type,
        );
        Some(suggestions)
    }

    #[cfg(test)]
    fn collect_local_rowtype_member_suggestions_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
        table_name: &str,
        columns: &[&str],
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);
        let sources = Self::local_rowtype_member_sources_for_qualifier(
            qualifier,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        );
        if sources.is_empty() {
            return None;
        }

        let mut data = IntellisenseData::new();
        data.set_columns_for_table(
            table_name,
            columns.iter().map(|column| (*column).to_string()).collect(),
        );
        Some(data.get_column_suggestions(prefix, Some(&sources)))
    }

    #[cfg(test)]
    fn collect_local_rowtype_member_suggestions_with_expected_type_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
        table_name: &str,
        columns_with_types: &[(&str, &str)],
        expected_type: ExpectedOperandTypes,
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);
        let sources = Self::local_rowtype_member_sources_for_qualifier(
            qualifier,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        );
        if sources.is_empty() {
            return None;
        }

        let mut data = IntellisenseData::new();
        let columns = columns_with_types
            .iter()
            .map(|(column, _)| (*column).to_string())
            .collect();
        data.set_columns_for_table(table_name, columns);
        data.set_column_meta_for_table(
            table_name,
            columns_with_types
                .iter()
                .map(|(column, type_display)| {
                    (
                        (*column).to_string(),
                        crate::ui::intellisense::ColumnMeta {
                            type_display: (*type_display).to_string(),
                            nullable: true,
                            is_primary_key: false,
                        },
                    )
                })
                .collect(),
        );
        let mut suggestions = data.get_column_suggestions(prefix, Some(&sources));
        suggestions.retain(|suggestion| {
            Self::column_suggestion_matches_expected_operand_type(
                &data,
                suggestion,
                Some(&sources),
                expected_type,
            )
        });
        Some(suggestions)
    }

    #[cfg(test)]
    fn collect_local_member_suggestions_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
        table_name: &str,
        columns: &[&str],
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);

        let local_record_member_suggestions = Self::collect_local_record_member_suggestions(
            qualifier,
            prefix,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        );
        let local_rowtype_member_sources = Self::local_rowtype_member_sources_for_qualifier(
            qualifier,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        );

        let local_record_member_suggestions =
            local_record_member_suggestions.unwrap_or_default();
        let local_rowtype_member_suggestions = if !local_rowtype_member_sources.is_empty() {
            let mut data = IntellisenseData::new();
            data.set_columns_for_table(
                table_name,
                columns.iter().map(|column| (*column).to_string()).collect(),
            );
            data.get_column_suggestions(prefix, Some(&local_rowtype_member_sources))
        } else {
            Vec::new()
        };

        let suggestions = Self::merge_suggestions_with_context_aliases(
            local_record_member_suggestions,
            local_rowtype_member_suggestions,
            false,
        );
        if suggestions.is_empty() {
            None
        } else {
            Some(suggestions)
        }
    }

    #[cfg(test)]
    fn collect_local_rowtype_member_suggestions_with_default_for_test(
        script_with_cursor: &str,
        qualifier: &str,
        prefix: &str,
        default_qualifier: &str,
        table_name: &str,
        columns: &[&str],
    ) -> Option<Vec<String>> {
        const CURSOR_MARKER: &str = "__CODEX_CURSOR__";

        let cursor = script_with_cursor.find(CURSOR_MARKER)?;
        let sql = script_with_cursor.replacen(CURSOR_MARKER, "", 1);
        let (routine_cache, expanded) =
            Self::build_routine_symbol_cache_bundle_for_test(&sql, cursor);
        let analysis = Self::build_intellisense_analysis_from_routine_cache(
            &routine_cache,
            expanded.cursor_in_statement,
        );
        let word_start = cursor.saturating_sub(prefix.len());
        let raw_qualifier = Self::raw_qualifier_before_word_in_text(&sql, word_start);
        let sources = Self::local_rowtype_member_sources_for_qualifier(
            qualifier,
            expanded.cursor_in_statement,
            raw_qualifier.as_deref(),
            &analysis,
        );
        if sources.is_empty() {
            return None;
        }

        let relation_member = table_name.rsplit('.').next().unwrap_or(table_name);
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some(default_qualifier.to_string()));
        data.set_relation_members_for_qualifier(
            default_qualifier,
            vec![relation_member.to_string()],
        );
        data.set_columns_for_table(
            table_name,
            columns.iter().map(|column| (*column).to_string()).collect(),
        );
        Some(data.get_column_suggestions(prefix, Some(&sources)))
    }
}
