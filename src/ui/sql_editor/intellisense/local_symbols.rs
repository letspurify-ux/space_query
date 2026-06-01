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
    parameter_names: Vec<String>,
    body_starts_immediately: bool,
}

#[derive(Clone)]
struct ParsedPackageBodyHeader {
    body_keyword_idx: usize,
    decl_start_idx: usize,
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

        if Self::expanded_statement_contains_ascii_case_insensitive(&expanded.text, "BEGIN")
            || Self::expanded_statement_contains_ascii_case_insensitive(&expanded.text, "DECLARE")
            || Self::expanded_statement_contains_ascii_case_insensitive(
                &expanded.text,
                "PACKAGE BODY",
            )
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

        IntellisenseAnalysis {
            statement_start: routine_cache.statement_start,
            statement_end: routine_cache.statement_end,
            context: Arc::new(context),
            local_scopes: routine_cache.local_scopes.clone(),
            local_symbols: routine_cache.local_symbols.clone(),
            text_bind_names: routine_cache.text_bind_names.clone(),
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
        let prefix_upper = prefix.to_ascii_uppercase();
        let cursor_in_statement = cursor_in_statement.min(
            analysis
                .statement_end
                .saturating_sub(analysis.statement_start),
        );
        let mut suggestions = Vec::new();
        let mut seen_local_symbols = HashSet::new();
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
            if symbol.declared_at > cursor_in_statement {
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
            if seen_local_symbols.insert(symbol.upper.as_str()) {
                scoped_suggestions[scope_rank].push(symbol.name.clone());
            }
        }

        for bucket in scoped_suggestions {
            suggestions.extend(bucket);
        }

        for name in analysis.text_bind_names.iter() {
            let upper = name.to_ascii_uppercase();
            if !Self::local_symbol_matches_prefix(name, &upper, &prefix_upper) {
                continue;
            }
            if !seen_local_symbols.contains(upper.as_str()) && seen_dynamic_symbols.insert(upper) {
                suggestions.push(name.clone());
            }
        }

        for name in session_bind_names {
            let upper = name.to_ascii_uppercase();
            if !Self::local_symbol_matches_prefix(name, &upper, &prefix_upper) {
                continue;
            }
            if !seen_local_symbols.contains(upper.as_str()) && seen_dynamic_symbols.insert(upper) {
                suggestions.push(name.clone());
            }
        }

        suggestions
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
        merged.retain(|value| seen.insert(value.to_ascii_uppercase()));
        merged.truncate(MAX_MERGED_SUGGESTIONS);
        merged
    }

    fn local_symbol_matches_prefix(name: &str, upper: &str, prefix_upper: &str) -> bool {
        if prefix_upper.is_empty() {
            return true;
        }

        upper.starts_with(prefix_upper) && !name.eq_ignore_ascii_case(prefix_upper)
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
        let mut pending_loop_var = None::<String>;
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
                            for name in parsed.parameter_names {
                                Self::push_local_symbol(
                                    &mut symbols,
                                    &mut seen_symbol_keys,
                                    scope_id,
                                    name,
                                    scope_start,
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
                                for name in Self::extract_mysql_declaration_symbols_from_item(item) {
                                    Self::push_local_symbol(
                                        &mut symbols,
                                        &mut seen_symbol_keys,
                                        scope_id,
                                        name,
                                        declared_at,
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
                            pending_loop_var = Self::parse_for_loop_variable(token_spans, idx);
                        }
                        "LOOP" => {
                            if prev_is_end {
                                idx += 1;
                                continue;
                            }

                            let scope_id = pending_loop_var.take().map(|name| {
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
                                Self::push_local_symbol(
                                    &mut symbols,
                                    &mut seen_symbol_keys,
                                    next_scope_id,
                                    name,
                                    token.end,
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
            if let Some(name) = Self::extract_declaration_symbol_from_item(item) {
                let declared_at = Self::declaration_item_declared_at(item);
                Self::push_local_symbol(
                    symbols,
                    seen_symbol_keys,
                    scope_id,
                    name,
                    declared_at,
                );
            }

            idx = item_end;
        }
    }

    fn extract_declaration_symbol_from_item(item: &[SqlTokenSpan]) -> Option<String> {
        let first_idx = item
            .iter()
            .position(|span| !matches!(span.token, SqlToken::Comment(_)))?;
        let first_word = Self::token_word(&item[first_idx].token)?;
        let first_upper = first_word.to_ascii_uppercase();

        match first_upper.as_str() {
            "PROCEDURE" | "FUNCTION" | "TYPE" | "SUBTYPE" | "PRAGMA" | "EXCEPTION" => {
                return None;
            }
            "CURSOR" => {
                let cursor_name = item[first_idx + 1..]
                    .iter()
                    .find_map(|span| Self::token_word(&span.token))
                    .and_then(Self::local_identifier_from_word)?;
                return Some(cursor_name);
            }
            _ => {}
        }

        let name = Self::local_identifier_from_word(first_word)?;

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

        Some(name)
    }

    fn extract_mysql_declaration_symbols_from_item(item: &[SqlTokenSpan]) -> Vec<String> {
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
                    if let Some(name) = Self::local_identifier_from_word(word) {
                        names.push(name);
                        expecting_name = false;
                    } else {
                        break;
                    }
                }
                SqlToken::Word(word) => {
                    let upper = word.to_ascii_uppercase();
                    if upper == "CURSOR" || upper == "CONDITION" {
                        return names;
                    }
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

    fn parse_for_loop_variable(tokens: &[SqlTokenSpan], idx: usize) -> Option<String> {
        let name_idx = Self::next_meaningful_token_idx(tokens, idx + 1)?;
        let name =
            Self::token_word(&tokens[name_idx].token).and_then(Self::local_identifier_from_word)?;
        let in_idx = Self::next_meaningful_token_idx(tokens, name_idx + 1)?;
        let in_word = Self::token_word(&tokens[in_idx].token)?;
        if !in_word.eq_ignore_ascii_case("IN") {
            return None;
        }
        Some(name)
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
        let mut parameter_names = Vec::new();
        let mut saw_mysql_returns = false;
        if scan_idx < tokens.len() && Self::token_symbol_at(tokens, scan_idx, "(") {
            let (close_idx, names) = Self::extract_parameter_names(tokens, scan_idx)?;
            parameter_names = names;
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
                        parameter_names,
                        body_starts_immediately: false,
                    });
                }
                SqlToken::Word(word) if paren_depth == 0 && word.eq_ignore_ascii_case("RETURNS") => {
                    saw_mysql_returns = true;
                }
                SqlToken::Word(word) if paren_depth == 0 && word.eq_ignore_ascii_case("BEGIN") => {
                    return Some(ParsedRoutineHeader {
                        body_keyword_idx: scan_idx,
                        decl_start_idx: scan_idx,
                        parameter_names,
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
                        parameter_names,
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

    fn extract_parameter_names(
        tokens: &[SqlTokenSpan],
        open_idx: usize,
    ) -> Option<(usize, Vec<String>)> {
        if !Self::token_symbol_at(tokens, open_idx, "(") {
            return None;
        }

        let mut idx = open_idx + 1;
        let mut depth = 1usize;
        let mut item_start = idx;
        let mut names = Vec::new();

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
                            if let Some(name) =
                                Self::extract_parameter_name_from_item(&tokens[item_start..idx])
                            {
                                names.push(name);
                            }
                        }
                        return Some((idx, names));
                    }
                }
                SqlToken::Symbol(sym) if sym == "," && depth == 1 => {
                    if item_start < idx {
                        if let Some(name) =
                            Self::extract_parameter_name_from_item(&tokens[item_start..idx])
                        {
                            names.push(name);
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

    fn extract_parameter_name_from_item(item: &[SqlTokenSpan]) -> Option<String> {
        item.iter()
            .filter_map(|span| Self::token_word(&span.token))
            .skip_while(|word| Self::is_parameter_mode_keyword(word))
            .find_map(Self::local_identifier_from_word)
    }

    fn is_parameter_mode_keyword(word: &str) -> bool {
        matches!(word.to_ascii_uppercase().as_str(), "IN" | "OUT" | "INOUT")
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

    fn push_local_symbol(
        symbols: &mut Vec<LocalSymbolEntry>,
        seen_symbol_keys: &mut HashSet<(usize, usize, String)>,
        scope_id: usize,
        name: String,
        declared_at: usize,
    ) {
        let upper = name.to_ascii_uppercase();
        if !seen_symbol_keys.insert((scope_id, declared_at, upper.clone())) {
            return;
        }
        symbols.push(LocalSymbolEntry {
            scope_id,
            upper,
            name,
            declared_at,
        });
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
            "",
            expanded.cursor_in_statement,
            &analysis,
            &session_bind_names,
        )
    }
}
