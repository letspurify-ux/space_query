#[derive(Clone)]
struct AsyncIntellisenseParseResult {
    analysis: IntellisenseAnalysis,
    routine_cache: RoutineSymbolCacheEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectListWildcardMode {
    None,
    Unqualified,
    Qualified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualifiedCompletionMode {
    RelationColumns,
    RelationMembers,
    ObjectMembers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedObjectSuggestionKind {
    Any,
    Routine,
    Executable,
    RelationOrSequence,
    Table,
    View,
    MaterializedView,
    Type,
    Trigger,
    Event,
    Index,
    Procedure,
    Function,
    Package,
    Sequence,
    Synonym,
    PublicSynonym,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClauseCompletionPolicy {
    restrict_to_relation_columns: bool,
    select_list_wildcard_mode: SelectListWildcardMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoJoinRelationSegment {
    text: String,
    quoted_dotted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoJoinTableMatchKey {
    full: String,
    short: String,
    allow_short_match: bool,
}

impl ClauseCompletionPolicy {
    fn for_phase(phase: intellisense_context::SqlPhase, has_qualifier: bool) -> Self {
        let restrict_to_relation_columns = matches!(
            phase,
            intellisense_context::SqlPhase::CteColumnList
                | intellisense_context::SqlPhase::DerivedAliasColumnList
                | intellisense_context::SqlPhase::ConflictTargetList
                | intellisense_context::SqlPhase::JoinUsingColumnList
                | intellisense_context::SqlPhase::RecursiveCteColumnList
                | intellisense_context::SqlPhase::DmlSetTargetList
                | intellisense_context::SqlPhase::InsertColumnList
                | intellisense_context::SqlPhase::MergeInsertColumnList
                | intellisense_context::SqlPhase::DmlReturningList
                | intellisense_context::SqlPhase::LockingColumnList
        );
        let select_list_wildcard_mode =
            if matches!(phase, intellisense_context::SqlPhase::SelectList) {
                if has_qualifier {
                    SelectListWildcardMode::Qualified
                } else {
                    SelectListWildcardMode::Unqualified
                }
            } else {
                SelectListWildcardMode::None
            };

        Self {
            restrict_to_relation_columns,
            select_list_wildcard_mode,
        }
    }
}

impl SqlEditorWidget {
    fn context_suppresses_completion(context: SqlContext) -> bool {
        matches!(context, SqlContext::GeneratedName)
    }

    pub(super) fn trigger_intellisense(
        editor: &TextEditor,
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) {
        let request_generation = runtime.next_parse_generation();
        let buffer_revision = runtime.current_buffer_revision();
        let (cursor_pos, cursor_pos_usize) = Self::editor_cursor_position(editor, buffer);
        let (prefix, word_start, _) = Self::word_at_cursor(buffer, text_shadow, cursor_pos);
        let qualifier = Self::qualifier_before_word(buffer, text_shadow, word_start);
        let raw_qualifier = Self::raw_qualifier_before_word(buffer, text_shadow, word_start);
        // Avoid blocking the UI thread on the connection mutex (which the
        // schema refresh worker or a running query may be holding). Fall back
        // to the last observed db_type; it only changes on (re)connect.
        let preferred_db_type = match connection.try_lock() {
            Ok(conn_guard) => {
                let db_type = conn_guard.db_type();
                runtime.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let db_type = poisoned.into_inner().db_type();
                runtime.update_cached_db_type(db_type);
                db_type
            }
            Err(std::sync::TryLockError::WouldBlock) => runtime.cached_db_type(),
        };
        let should_hide_after_statement_terminator = prefix.is_empty()
            && qualifier.is_none()
            && Self::non_whitespace_char_before_cursor(buffer, text_shadow, cursor_pos)
                == Some(';');

        if should_hide_after_statement_terminator {
            intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            runtime.clear_ui_tracking();
            return;
        }

        let snapshot = Arc::new(IntellisenseTriggerSnapshot {
            request_generation,
            buffer_revision,
            cursor_pos,
            cursor_pos_usize,
            preferred_db_type,
            prefix,
            word_start,
            qualifier,
            raw_qualifier,
        });

        let cached_context = runtime.parse_cache().and_then(|entry| {
            (entry.buffer_revision == snapshot.buffer_revision
                && entry.cursor_pos == snapshot.cursor_pos)
                .then_some(entry.analysis.clone())
        });

        if let Some(analysis) = cached_context {
            Self::apply_intellisense_with_context(
                editor,
                intellisense_data,
                intellisense_popup,
                column_sender,
                connection,
                runtime,
                snapshot.as_ref(),
                analysis.as_ref(),
            );
            return;
        }

        // Cache miss means full parse is pending on a worker.
        // Hide stale popup/completion state to avoid applying outdated candidates.
        Self::clear_intellisense_ui_state(intellisense_popup, runtime);

        Self::queue_async_intellisense_parse(
            editor,
            text_shadow,
            intellisense_data,
            intellisense_popup,
            column_sender,
            connection,
            runtime,
            snapshot.clone(),
        );
    }

    #[cfg(test)]
    fn analyze_statement_context(
        statement_text: &str,
        cursor_in_statement: usize,
    ) -> intellisense_context::CursorContext {
        type CachedStatementTokens = (Arc<[usize]>, Arc<[SqlToken]>);

        static TOKENIZED_STATEMENT_CACHE: OnceLock<Mutex<HashMap<String, CachedStatementTokens>>> =
            OnceLock::new();

        let cache = TOKENIZED_STATEMENT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let (token_ends, statement_tokens) = {
            let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(entry) = guard.get(statement_text) {
                entry.clone()
            } else {
                let full_token_spans = super::query_text::tokenize_sql_spanned(statement_text);
                let token_ends: Arc<[usize]> = full_token_spans
                    .iter()
                    .map(|span| span.end)
                    .collect::<Vec<_>>()
                    .into();
                let statement_tokens: Arc<[SqlToken]> = full_token_spans
                    .into_iter()
                    .map(|span| span.token)
                    .collect::<Vec<_>>()
                    .into();
                let entry = (token_ends, statement_tokens);
                guard.insert(statement_text.to_string(), entry.clone());
                entry
            }
        };
        let split_idx = token_ends.partition_point(|end| *end <= cursor_in_statement);
        intellisense_context::analyze_cursor_context_arc(statement_tokens, split_idx)
    }

    fn is_intellisense_snapshot_current(
        editor: &TextEditor,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
    ) -> bool {
        if editor.was_deleted() {
            return false;
        }

        if editor.insert_position() != snapshot.cursor_pos {
            return false;
        }

        runtime.current_buffer_revision() == snapshot.buffer_revision
    }

    fn is_intellisense_parse_generation_current(
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
    ) -> bool {
        runtime.current_parse_generation() == snapshot.request_generation
    }

    fn clear_intellisense_ui_state(
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) {
        intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
        runtime.clear_ui_tracking();
    }

    fn clear_intellisense_state_for_external_hide(runtime: &Arc<IntellisenseRuntimeState>) {
        Self::invalidate_and_clear_pending_intellisense_state(runtime);
    }

    fn should_ignore_external_hide_click(popup_visible: bool, click_inside_popup: bool) -> bool {
        popup_visible && click_inside_popup
    }

    fn should_hide_popup_on_unfocus(popup_visible: bool, pointer_inside_popup: bool) -> bool {
        popup_visible && !pointer_inside_popup
    }

    fn schedule_deferred_unfocus_popup_hide(
        editor: TextEditor,
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        pointer_x: i32,
        pointer_y: i32,
        retries_left: u8,
    ) {
        app::add_timeout3(0.0, move |_| {
            if editor.was_deleted() {
                return;
            }

            if matches!(
                runtime.popup_transition_state(),
                IntellisensePopupTransitionState::Showing
            ) {
                if retries_left > 0 {
                    Self::schedule_deferred_unfocus_popup_hide(
                        editor.clone(),
                        intellisense_popup.clone(),
                        runtime.clone(),
                        pointer_x,
                        pointer_y,
                        retries_left - 1,
                    );
                }
                return;
            }

            if editor.has_focus() {
                return;
            }
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let popup_visible = popup.is_visible();
            let pointer_inside_popup = popup_visible && popup.contains_point(pointer_x, pointer_y);
            if !Self::should_hide_popup_on_unfocus(popup_visible, pointer_inside_popup) {
                return;
            }
            popup.hide();
            drop(popup);
            Self::clear_intellisense_state_for_external_hide(&runtime);
        });
    }

    fn schedule_deferred_outside_click_popup_hide(
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        click_x: i32,
        click_y: i32,
        retries_left: u8,
    ) {
        app::add_timeout3(0.0, move |_| {
            if matches!(
                runtime.popup_transition_state(),
                IntellisensePopupTransitionState::Showing
            ) {
                if retries_left > 0 {
                    Self::schedule_deferred_outside_click_popup_hide(
                        intellisense_popup.clone(),
                        runtime.clone(),
                        click_x,
                        click_y,
                        retries_left - 1,
                    );
                }
                return;
            }
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let popup_visible = popup.is_visible();
            if !popup_visible {
                return;
            }
            let click_inside_popup = popup.contains_point(click_x, click_y);
            if Self::should_ignore_external_hide_click(popup_visible, click_inside_popup) {
                return;
            }
            popup.hide();
            drop(popup);
            Self::clear_intellisense_state_for_external_hide(&runtime);
        });
    }

    fn invalidate_and_clear_pending_intellisense_state(runtime: &Arc<IntellisenseRuntimeState>) {
        runtime.clear_ui_tracking();
        Self::invalidate_keyup_debounce_with_parse_generation(runtime, true);
    }

    fn cancel_intellisense_on_escape_keydown(
        popup_visible: bool,
        runtime: &Arc<IntellisenseRuntimeState>,
    ) -> bool {
        Self::invalidate_and_clear_pending_intellisense_state(runtime);
        popup_visible
    }

    fn queue_async_intellisense_parse(
        editor: &TextEditor,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: Arc<IntellisenseTriggerSnapshot>,
    ) {
        let (parse_sender, parse_receiver) =
            mpsc::channel::<Result<AsyncIntellisenseParseResult, String>>();
        let parse_receiver = Arc::new(Mutex::new(parse_receiver));
        let snapshot_for_thread = snapshot.clone();
        let text_shadow_for_thread = text_shadow.clone();
        let routine_symbol_cache_for_thread = runtime.routine_symbol_cache_handle();
        let spawn_result = thread::Builder::new()
            .name("intellisense-parse-worker".to_string())
            .spawn(move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        let (expanded_statement, text_bind_names) =
                            Self::expanded_statement_window_and_text_binds_from_shadow(
                                &text_shadow_for_thread,
                                snapshot_for_thread.cursor_pos_usize,
                                Some(snapshot_for_thread.preferred_db_type),
                            );
                    let routine_cache = {
                        let cache = routine_symbol_cache_for_thread
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        cache
                            .iter()
                            .find(|entry| {
                                entry.buffer_revision == snapshot_for_thread.buffer_revision
                                    && entry.statement_start == expanded_statement.statement_start
                                    && entry.statement_end == expanded_statement.statement_end
                            })
                            .cloned()
                    }
                    .unwrap_or_else(|| {
                        Self::build_routine_symbol_cache_entry(
                            snapshot_for_thread.buffer_revision,
                            &expanded_statement,
                            text_bind_names,
                        )
                    });
                    let analysis = Self::build_intellisense_analysis_from_routine_cache(
                        &routine_cache,
                        expanded_statement.cursor_in_statement,
                    );

                    AsyncIntellisenseParseResult {
                        analysis,
                        routine_cache,
                    }
                }));

                match result {
                    Ok(parsed) => {
                        let _ = parse_sender.send(Ok(parsed));
                    }
                    Err(payload) => {
                        let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                        crate::utils::logging::log_error(
                            "sql_editor::intellisense::parse_worker",
                            &format!("parse worker panicked: {panic_msg}"),
                        );
                        let _ = parse_sender.send(Err(format!("Internal error: {panic_msg}")));
                    }
                }
                app::awake();
            });

        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::parse_worker",
                &format!("failed to spawn parse worker: {err}"),
            );
            if Self::is_intellisense_parse_generation_current(runtime, snapshot.as_ref())
                && Self::is_intellisense_snapshot_current(editor, runtime, snapshot.as_ref())
            {
                Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            }
            return;
        }

        let editor_for_poll = editor.clone();
        let intellisense_data_for_poll = intellisense_data.clone();
        let intellisense_popup_for_poll = intellisense_popup.clone();
        let column_sender_for_poll = column_sender.clone();
        let connection_for_poll = connection.clone();
        let runtime_for_poll = runtime.clone();
        app::add_timeout3(0.0, move |_| {
            Self::poll_async_intellisense_parse(
                editor_for_poll.clone(),
                intellisense_data_for_poll.clone(),
                intellisense_popup_for_poll.clone(),
                column_sender_for_poll.clone(),
                connection_for_poll.clone(),
                runtime_for_poll.clone(),
                snapshot.clone(),
                parse_receiver.clone(),
            );
        });
    }

    fn poll_async_intellisense_parse(
        editor: TextEditor,
        intellisense_data: Arc<Mutex<IntellisenseData>>,
        intellisense_popup: Arc<Mutex<IntellisensePopup>>,
        column_sender: mpsc::Sender<ColumnLoadUpdate>,
        connection: SharedConnection,
        runtime: Arc<IntellisenseRuntimeState>,
        snapshot: Arc<IntellisenseTriggerSnapshot>,
        parse_receiver: Arc<Mutex<mpsc::Receiver<Result<AsyncIntellisenseParseResult, String>>>>,
    ) {
        if editor.was_deleted() {
            return;
        }
        if !Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref()) {
            return;
        }

        let recv_result = {
            let receiver = parse_receiver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            receiver.try_recv()
        };

        match recv_result {
            Ok(Ok(parsed)) => {
                if !Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    || !Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    return;
                }
                runtime.set_routine_symbol_cache(parsed.routine_cache.clone());
                let parsed = Arc::new(parsed.analysis);
                runtime.set_parse_cache(Some(IntellisenseParseCacheEntry {
                    buffer_revision: snapshot.buffer_revision,
                    cursor_pos: snapshot.cursor_pos,
                    analysis: parsed.clone(),
                }));

                Self::apply_intellisense_with_context(
                    &editor,
                    &intellisense_data,
                    &intellisense_popup,
                    &column_sender,
                    &connection,
                    &runtime,
                    snapshot.as_ref(),
                    parsed.as_ref(),
                );
            }
            Ok(Err(message)) => {
                crate::utils::logging::log_error(
                    "sql_editor::intellisense::parse_worker",
                    &format!("failed to parse intellisense context: {message}"),
                );
                if Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    && Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    Self::clear_intellisense_ui_state(&intellisense_popup, &runtime);
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                app::add_timeout3(INTELLISENSE_PARSE_POLL_INTERVAL_SECONDS, move |_| {
                    Self::poll_async_intellisense_parse(
                        editor.clone(),
                        intellisense_data.clone(),
                        intellisense_popup.clone(),
                        column_sender.clone(),
                        connection.clone(),
                        runtime.clone(),
                        snapshot.clone(),
                        parse_receiver.clone(),
                    );
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                if Self::is_intellisense_parse_generation_current(&runtime, snapshot.as_ref())
                    && Self::is_intellisense_snapshot_current(&editor, &runtime, snapshot.as_ref())
                {
                    Self::clear_intellisense_ui_state(&intellisense_popup, &runtime);
                }
            }
        }
    }

    fn apply_intellisense_with_context(
        editor: &TextEditor,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        runtime: &Arc<IntellisenseRuntimeState>,
        snapshot: &IntellisenseTriggerSnapshot,
        analysis: &IntellisenseAnalysis,
    ) {
        let deep_ctx = analysis.context.as_ref();
        let qualifier = snapshot.qualifier.as_deref();
        let context =
            Self::classify_intellisense_context(deep_ctx, deep_ctx.statement_tokens.as_ref());
        if Self::context_suppresses_completion(context) {
            Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            return;
        }
        let completion_policy =
            ClauseCompletionPolicy::for_phase(deep_ctx.phase, qualifier.is_some());
        let restrict_to_relation_columns = completion_policy.restrict_to_relation_columns;
        let cursor_in_statement = snapshot
            .cursor_pos_usize
            .saturating_sub(analysis.statement_start)
            .min(
                analysis
                    .statement_end
                    .saturating_sub(analysis.statement_start),
            );
        let session_bind_names = if qualifier.is_none()
            && !matches!(context, SqlContext::TableName)
            && !restrict_to_relation_columns
        {
            Self::session_bind_names(connection)
        } else {
            Vec::new()
        };
        let local_suggestions = if qualifier.is_none()
            && !matches!(context, SqlContext::TableName)
            && !restrict_to_relation_columns
        {
            Self::collect_local_symbol_suggestions(
                &snapshot.prefix,
                cursor_in_statement,
                analysis,
                &session_bind_names,
            )
        } else {
            Vec::new()
        };
        let local_record_member_suggestions = qualifier
            .and_then(|qualifier| {
                Self::collect_local_record_member_suggestions(
                    qualifier,
                    &snapshot.prefix,
                    cursor_in_statement,
                    snapshot.raw_qualifier.as_deref(),
                    analysis,
                )
            });
        let has_resolved_local_record_member_scope = local_record_member_suggestions.is_some();
        let local_rowtype_member_sources = qualifier
            .map(|qualifier| {
                Self::local_rowtype_member_sources_for_qualifier(
                    qualifier,
                    cursor_in_statement,
                    snapshot.raw_qualifier.as_deref(),
                    analysis,
                )
            })
            .unwrap_or_default();
        for source in &local_rowtype_member_sources {
            Self::request_table_columns(source, intellisense_data, column_sender, connection);
        }
        let local_rowtype_member_suggestions = if !local_rowtype_member_sources.is_empty() {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get_column_suggestions(&snapshot.prefix, Some(&local_rowtype_member_sources))
        } else {
            Vec::new()
        };
        let has_local_record_member_scope =
            has_resolved_local_record_member_scope || !local_rowtype_member_sources.is_empty();
        let local_record_member_suggestions =
            local_record_member_suggestions.unwrap_or_default();
        let local_rowtype_column_tables = local_rowtype_member_sources.clone();
        let qualified_completion_mode = if has_local_record_member_scope {
            None
        } else {
            qualifier.and_then(|qualifier| {
                let data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::resolve_qualified_completion_mode(qualifier, context, deep_ctx, &data)
            })
        };
        let qualified_mode_uses_members = matches!(
            qualified_completion_mode,
            Some(QualifiedCompletionMode::RelationMembers | QualifiedCompletionMode::ObjectMembers)
        );
        let column_tables = if has_local_record_member_scope || qualified_mode_uses_members {
            Vec::new()
        } else {
            Self::resolve_column_tables_for_context(qualifier, deep_ctx)
        };
        let include_columns = !has_local_record_member_scope
            && (matches!(
                qualified_completion_mode,
                Some(QualifiedCompletionMode::RelationColumns)
            ) || (qualified_completion_mode.is_none()
                && (qualifier.is_some()
                    || matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll))));
        let comparison_lookup_tables = if has_local_record_member_scope || qualified_mode_uses_members
        {
            Vec::new()
        } else {
            Self::comparison_lookup_tables_for_context(qualifier, deep_ctx)
        };
        let qualified_member_suggestions = match (qualifier, qualified_completion_mode) {
            (Some(qualifier), Some(QualifiedCompletionMode::RelationMembers)) => {
                let mut data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::expected_relation_member_suggestions_for_qualifier(
                    &mut data,
                    qualifier,
                    &snapshot.prefix,
                    deep_ctx,
                )
            }
            (Some(qualifier), Some(QualifiedCompletionMode::ObjectMembers)) => {
                let mut data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::expected_member_suggestions_for_qualifier(
                    &mut data,
                    qualifier,
                    &snapshot.prefix,
                    deep_ctx,
                )
            }
            _ => Vec::new(),
        };
        let expected_keyword_suggestions = if qualifier.is_none()
            && !restrict_to_relation_columns
            && !matches!(context, SqlContext::VariableName | SqlContext::BindValue)
        {
            Self::collect_expected_keyword_suggestions(&snapshot.prefix, deep_ctx)
        } else {
            Vec::new()
        };
        let expected_object_suggestions = if qualifier.is_none()
            && !restrict_to_relation_columns
            && !matches!(
                context,
                SqlContext::VariableName
                    | SqlContext::BindValue
                    | SqlContext::ColumnName
                    | SqlContext::ColumnOrAll
                    | SqlContext::TableName
            ) {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::collect_expected_object_suggestions(&mut data, &snapshot.prefix, deep_ctx)
        } else {
            Vec::new()
        };

        let allow_empty_prefix = qualifier.is_some()
            || include_columns
            || matches!(context, SqlContext::TableName)
            || !local_suggestions.is_empty()
            || has_local_record_member_scope
            || !qualified_member_suggestions.is_empty()
            || !expected_keyword_suggestions.is_empty()
            || !expected_object_suggestions.is_empty();
        if snapshot.prefix.is_empty() && !allow_empty_prefix {
            // Context no longer allows completion for empty prefix, so hide
            // stale popup state immediately.
            Self::clear_intellisense_ui_state(intellisense_popup, runtime);
            return;
        }

        let mut virtual_wildcard_dependencies: HashMap<String, Vec<String>> = HashMap::new();
        if include_columns {
            let mut virtual_table_columns: HashMap<String, Vec<String>> = HashMap::new();
            for cte in &deep_ctx.ctes {
                let (columns, wildcard_tables) = Self::collect_cte_virtual_columns_for_completion(
                    deep_ctx,
                    cte,
                    &virtual_table_columns,
                    intellisense_data,
                    column_sender,
                    connection,
                );
                if !wildcard_tables.is_empty() {
                    virtual_wildcard_dependencies.insert(cte.name.to_uppercase(), wildcard_tables);
                }
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut virtual_table_columns,
                        &cte.name,
                        columns,
                    );
                }
            }

            for subq in &deep_ctx.subqueries {
                if let Some(columns) =
                    Self::explicit_subquery_columns_for_completion(deep_ctx, subq)
                {
                    Self::insert_virtual_table_columns(
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
                let body_ctx =
                    intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
                let mut body_virtual_table_columns = virtual_table_columns.clone();
                for cte in &body_ctx.ctes {
                    let (nested_columns, _) = Self::collect_cte_virtual_columns_for_completion(
                        &body_ctx,
                        cte,
                        &body_virtual_table_columns,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                    if !nested_columns.is_empty() {
                        Self::insert_virtual_table_columns(
                            &mut body_virtual_table_columns,
                            &cte.name,
                            nested_columns,
                        );
                    }
                }
                let body_local_tables =
                    intellisense_context::collect_tables_in_statement(body_tokens);
                let (columns, wildcard_tables) =
                    Self::collect_virtual_relation_columns_for_completion(
                        body_tokens,
                        &body_local_tables,
                        &deep_ctx.tables_in_scope,
                        &body_virtual_table_columns,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                if !wildcard_tables.is_empty() {
                    virtual_wildcard_dependencies
                        .insert(subq.alias.to_uppercase(), wildcard_tables);
                }
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut virtual_table_columns,
                        &subq.alias,
                        columns,
                    );
                }
            }
            intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .replace_virtual_table_columns(virtual_table_columns);

            for table in &column_tables {
                let is_virtual = deep_ctx
                    .ctes
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(table))
                    || deep_ctx
                        .subqueries
                        .iter()
                        .any(|s| s.alias.eq_ignore_ascii_case(table));
                if !is_virtual {
                    Self::request_table_columns(
                        table,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                }
            }

            for table in &comparison_lookup_tables {
                Self::request_table_columns(table, intellisense_data, column_sender, connection);
            }
        }

        let columns_loading = if include_columns {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let loading_tables = if comparison_lookup_tables.is_empty() {
                column_tables.clone()
            } else {
                let mut merged_tables = column_tables.clone();
                for table in &comparison_lookup_tables {
                    if merged_tables
                        .iter()
                        .all(|existing| !existing.eq_ignore_ascii_case(table))
                    {
                        merged_tables.push(table.clone());
                    }
                }
                merged_tables
            };
            Self::has_column_loading_for_scope(
                include_columns,
                &loading_tables,
                &virtual_wildcard_dependencies,
                &data,
            )
        } else if !local_rowtype_column_tables.is_empty() {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::has_column_loading_for_scope(
                true,
                &local_rowtype_column_tables,
                &HashMap::new(),
                &data,
            )
        } else {
            false
        };

        let mut suggestions = if has_resolved_local_record_member_scope {
            Self::merge_suggestions_with_context_aliases(
                local_record_member_suggestions,
                local_rowtype_member_suggestions,
                false,
            )
        } else if !local_rowtype_member_sources.is_empty() {
            local_rowtype_member_suggestions
        } else if !qualified_member_suggestions.is_empty() {
            qualified_member_suggestions
        } else {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let column_scope = if !column_tables.is_empty() {
                Some(column_tables.as_slice())
            } else {
                None
            };
            if qualifier.is_none()
                && matches!(
                    deep_ctx.phase,
                    intellisense_context::SqlPhase::JoinUsingColumnList
                )
            {
                Self::collect_common_column_suggestions(&snapshot.prefix, &column_tables, &data)
            } else {
                Self::base_suggestions_for_context(
                    &mut data,
                    &snapshot.prefix,
                    qualifier,
                    column_scope,
                    include_columns,
                    context,
                    restrict_to_relation_columns,
                    Some(snapshot.preferred_db_type),
                )
            }
        };
        if !expected_object_suggestions.is_empty() {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                expected_object_suggestions,
                true,
            );
        }
        if !expected_keyword_suggestions.is_empty() {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                expected_keyword_suggestions,
                true,
            );
        }
        let comparison_suggestions = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            qualifier
                .map(|qualifier| {
                    Self::collect_qualified_condition_comparison_suggestions(
                        &data,
                        &snapshot.prefix,
                        qualifier,
                        deep_ctx,
                    )
                })
                .unwrap_or_default()
        };
        if !comparison_suggestions.is_empty() {
            suggestions = Self::merge_qualified_condition_comparison_suggestions(
                suggestions,
                comparison_suggestions,
                deep_ctx.phase,
            );
        }
        let wildcard_suggestions =
            Self::collect_clause_wildcard_suggestions(&snapshot.prefix, qualifier, deep_ctx);
        if !wildcard_suggestions.is_empty() {
            suggestions = Self::merge_suggestions_with_context_aliases(
                suggestions,
                wildcard_suggestions,
                true,
            );
        }
        if include_columns && qualifier.is_none() && !restrict_to_relation_columns {
            let derived_columns = Self::collect_derived_columns_for_context(deep_ctx);
            suggestions = Self::merge_suggestions_with_derived_columns(
                suggestions,
                &snapshot.prefix,
                derived_columns,
            );
        }
        let context_name_suggestions =
            if matches!(context, SqlContext::VariableName | SqlContext::BindValue)
                || restrict_to_relation_columns
            {
                Vec::new()
            } else {
                Self::collect_context_name_suggestions(&snapshot.prefix, deep_ctx, context)
            };
        let suggestions = Self::maybe_merge_suggestions_with_context_aliases(
            suggestions,
            context_name_suggestions,
            matches!(context, SqlContext::TableName),
            qualifier.is_some(),
        );
        let mut suggestions = if !local_suggestions.is_empty() {
            Self::prepend_local_symbol_suggestions(suggestions, local_suggestions)
        } else {
            suggestions
        };

        // Offer an FK-based join condition as the top suggestion when filling
        // in a JOIN ... ON clause between two related tables.
        if qualifier.is_none()
            && matches!(deep_ctx.phase, intellisense_context::SqlPhase::JoinCondition)
        {
            let real_tables: Vec<&intellisense_context::ScopedTableRef> = deep_ctx
                .tables_in_scope
                .iter()
                .filter(|table| !table.is_cte)
                .collect();
            if let [lefts @ .., right] = real_tables.as_slice() {
                if !lefts.is_empty() {
                    // Foreign keys are loaded lazily (only here, when a JOIN is
                    // actually being written) to keep ordinary column
                    // completion to a single query per table. A refresh fires
                    // when each load completes, re-running this branch.
                    for table in &real_tables {
                        Self::request_table_foreign_keys(
                            &table.name,
                            intellisense_data,
                            column_sender,
                            connection,
                        );
                    }
                    let condition = {
                        let data = intellisense_data
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        Self::build_auto_join_condition(&data, right, lefts)
                    };
                    if let Some(condition) = condition {
                        let matches_prefix = snapshot.prefix.is_empty()
                            || condition
                                .to_uppercase()
                                .starts_with(&snapshot.prefix.to_uppercase());
                        if matches_prefix && !suggestions.iter().any(|s| s == &condition) {
                            suggestions.insert(0, condition);
                        }
                    }
                }
            }
        }

        let should_refresh_when_columns_ready = include_columns && columns_loading;
        if should_refresh_when_columns_ready {
            runtime.set_pending_intellisense(Some(PendingIntellisense {
                cursor_pos: snapshot.cursor_pos,
            }));
        } else {
            runtime.clear_pending_intellisense();
        }

        if suggestions.is_empty() {
            intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            runtime.clear_completion_range();
            return;
        }

        let popup_width = Self::INTELLISENSE_POPUP_WIDTH;
        // Mirror IntellisensePopup's row height (font size + 6, min 20) so the
        // vertical clamp keeps the actual popup on screen.
        let row_h = (crate::ui::configured_ui_font_size() + 6).max(20);
        let popup_height = (suggestions.len().min(10) as i32) * row_h + 10;
        let (popup_x, popup_y) =
            Self::popup_screen_position(editor, snapshot.cursor_pos, popup_width, popup_height);
        struct PopupShowInProgressReset {
            runtime: Arc<IntellisenseRuntimeState>,
        }
        impl Drop for PopupShowInProgressReset {
            fn drop(&mut self) {
                self.runtime
                    .set_popup_transition_state(IntellisensePopupTransitionState::Idle);
            }
        }
        runtime.set_popup_transition_state(IntellisensePopupTransitionState::Showing);
        let _popup_show_reset = PopupShowInProgressReset {
            runtime: runtime.clone(),
        };
        let descriptions = if include_columns && !column_tables.is_empty() {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::build_column_descriptions(&data, &column_tables)
        } else {
            HashMap::new()
        };
        intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .show_suggestions_with_descriptions(suggestions, descriptions, popup_x, popup_y);
        let completion_start = if snapshot.prefix.is_empty() {
            snapshot.cursor_pos_usize
        } else {
            snapshot.word_start
        };
        runtime.set_completion_range(Some(IntellisenseCompletionRange::new(
            completion_start,
            snapshot.cursor_pos_usize,
        )));
        let mut editor = editor.clone();
        let _ = editor.take_focus();
    }

    fn base_suggestions_for_context(
        data: &mut IntellisenseData,
        prefix: &str,
        qualifier: Option<&str>,
        column_scope: Option<&[String]>,
        include_columns: bool,
        context: SqlContext,
        restrict_to_relation_columns: bool,
        db_type: Option<crate::db::DatabaseType>,
    ) -> Vec<String> {
        if qualifier.is_some() {
            return data.get_column_suggestions(prefix, column_scope);
        }

        if matches!(context, SqlContext::VariableName | SqlContext::BindValue) {
            return Vec::new();
        }

        if matches!(context, SqlContext::TableName) {
            return data.get_relation_suggestions(prefix);
        }

        if restrict_to_relation_columns {
            return data.get_column_suggestions(prefix, column_scope);
        }

        data.get_suggestions_for_db(
            prefix,
            include_columns,
            column_scope,
            false,
            matches!(context, SqlContext::ColumnName | SqlContext::ColumnOrAll),
            db_type,
        )
    }

    fn qualifier_matches_visible_relation_scope(
        qualifier: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> bool {
        deep_ctx.tables_in_scope.iter().any(|table_ref| {
            Self::completion_identifiers_match(&table_ref.name, qualifier)
                || table_ref
                    .alias
                    .as_deref()
                    .is_some_and(|alias| Self::completion_identifiers_match(alias, qualifier))
        }) || deep_ctx
            .ctes
            .iter()
            .any(|cte| Self::completion_identifiers_match(&cte.name, qualifier))
            || deep_ctx
                .subqueries
                .iter()
                .any(|subq| Self::completion_identifiers_match(&subq.alias, qualifier))
    }

    fn resolve_qualified_completion_mode(
        qualifier: &str,
        context: SqlContext,
        deep_ctx: &intellisense_context::CursorContext,
        data: &IntellisenseData,
    ) -> Option<QualifiedCompletionMode> {
        if matches!(context, SqlContext::TableName)
            && data.has_members_for_qualifier(qualifier, true)
        {
            return Some(QualifiedCompletionMode::RelationMembers);
        }

        if Self::qualifier_matches_visible_relation_scope(qualifier, deep_ctx)
            || data.is_known_relation(qualifier)
        {
            return Some(QualifiedCompletionMode::RelationColumns);
        }

        let resolved_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        if resolved_tables
            .iter()
            .any(|table| data.is_known_relation(table))
        {
            return Some(QualifiedCompletionMode::RelationColumns);
        }

        if data.has_members_for_qualifier(qualifier, false) {
            return Some(QualifiedCompletionMode::ObjectMembers);
        }

        None
    }

    fn previous_meaningful_words_upper(
        tokens: &[SqlToken],
        end: usize,
        max_words: usize,
    ) -> Vec<String> {
        if max_words == 0 {
            return Vec::new();
        }

        let mut words_rev = Vec::new();
        for token in tokens.get(..end).unwrap_or(tokens).iter().rev() {
            match token {
                SqlToken::Comment(_) => {}
                SqlToken::Word(word) => {
                    words_rev.push(word.to_ascii_uppercase());
                    if words_rev.len() >= max_words {
                        break;
                    }
                }
                SqlToken::Symbol(_) => {}
                _ => break,
            }
        }
        words_rev.reverse();
        words_rev
    }

    fn filter_expected_candidates(prefix: &str, candidates: &[&str]) -> Vec<String> {
        let prefix_upper = prefix.to_ascii_uppercase();
        let mut seen = HashSet::new();
        let mut suggestions = Vec::new();

        for candidate in candidates {
            let upper = candidate.to_ascii_uppercase();
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                suggestions.push((*candidate).to_string());
                if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                    break;
                }
            }
        }

        suggestions
    }

    fn collect_expected_keyword_suggestions(
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        const TOP_LEVEL_KEYWORDS: &[&str] = &[
            "SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "ALTER", "DROP",
            "BEGIN", "DECLARE", "CALL", "VALUES",
        ];
        const OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "USER",
            "PUBLIC",
        ];
        const CREATE_OBJECT_TYPE_KEYWORDS: &[&str] = &[
            "TABLE",
            "VIEW",
            "MATERIALIZED",
            "EDITIONING",
            "TYPE",
            "TRIGGER",
            "INDEX",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "SEQUENCE",
            "SYNONYM",
            "USER",
            "PUBLIC",
        ];
        const COMMENT_OBJECT_TYPE_KEYWORDS: &[&str] =
            &["COLUMN", "TABLE", "VIEW", "MATERIALIZED", "EDITIONING"];

        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let words = Self::previous_meaningful_words_upper(
            tokens,
            Self::expected_suggestion_context_end(tokens, cursor_token_len, !prefix.is_empty()),
            4,
        );

        let candidates: &[&str] = match words.as_slice() {
            [] => TOP_LEVEL_KEYWORDS,
            [.., last] if *last == "ORDER" || *last == "GROUP" || *last == "CONNECT" => &["BY"],
            [.., last] if *last == "START" => &["WITH"],
            [.., last] if *last == "DELETE" => &["FROM"],
            [.., last] if *last == "INSERT" || *last == "MERGE" => &["INTO"],
            [.., last]
                if matches!(
                    last.as_str(),
                    "LEFT" | "RIGHT" | "FULL" | "INNER" | "CROSS" | "NATURAL"
                ) =>
            {
                &["JOIN"]
            }
            [.., last] if matches!(last.as_str(), "UNION" | "INTERSECT" | "EXCEPT" | "MINUS") => {
                &["SELECT", "ALL"]
            }
            [.., last] if *last == "CREATE" => CREATE_OBJECT_TYPE_KEYWORDS,
            [.., last] if *last == "DROP" || *last == "ALTER" => OBJECT_TYPE_KEYWORDS,
            [.., prev, last]
                if matches!(prev.as_str(), "CREATE" | "DROP" | "ALTER" | "ON")
                    && *last == "MATERIALIZED" =>
            {
                &["VIEW"]
            }
            [.., a, b, c, d]
                if *a == "CREATE" && *b == "OR" && *c == "REPLACE" && *d == "MATERIALIZED" =>
            {
                &["VIEW"]
            }
            [.., prev, last]
                if matches!(prev.as_str(), "CREATE" | "ON") && *last == "EDITIONING" =>
            {
                &["VIEW"]
            }
            [.., a, b, c, d]
                if *a == "CREATE" && *b == "OR" && *c == "REPLACE" && *d == "EDITIONING" =>
            {
                &["VIEW"]
            }
            [.., prev, last]
                if !prefix.is_empty() && *prev == "DROP" && matches!(last.as_str(), "PACKAGE" | "TYPE") =>
            {
                &["BODY"]
            }
            [.., prev, last] if *prev == "DROP" && *last == "PUBLIC" => &["SYNONYM"],
            [.., prev, last] if *prev == "CREATE" && *last == "PUBLIC" => &["SYNONYM"],
            [.., prev, last] if *prev == "COMMENT" && *last == "ON" => {
                COMMENT_OBJECT_TYPE_KEYWORDS
            }
            [.., a, b, c] if *a == "CREATE" && *b == "OR" && *c == "REPLACE" => {
                CREATE_OBJECT_TYPE_KEYWORDS
            }
            [.., last] if *last == "TRUNCATE" || *last == "LOCK" || *last == "FLASHBACK" => {
                &["TABLE"]
            }
            [.., last] if *last == "COMMENT" => &["ON"],
            [.., last] if *last == "EXECUTE" => &["IMMEDIATE"],
            [.., last] if *last == "WHEN" => &["MATCHED", "NOT"],
            [.., prev, last] if *prev == "WHEN" && *last == "NOT" => &["MATCHED"],
            [.., prev, last] if *prev == "CREATE" && *last == "OR" => &["REPLACE"],
            _ => {
                if deep_ctx.cursor_token_len == 0 {
                    TOP_LEVEL_KEYWORDS
                } else {
                    &[]
                }
            }
        };

        Self::filter_expected_candidates(prefix, candidates)
    }

    fn expected_object_suggestion_kind(
        prefix: &str,
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Option<ExpectedObjectSuggestionKind> {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let words = Self::previous_meaningful_words_upper(
            tokens,
            Self::expected_suggestion_context_end(
                tokens,
                cursor_token_len,
                !prefix.is_empty() || qualifier.is_some(),
            ),
            16,
        );

        if let Some(kind) = Self::expected_grant_revoke_object_suggestion_kind(&words) {
            return Some(kind);
        }

        match words.as_slice() {
            [.., last] if matches!(last.as_str(), "CALL" | "EXEC" | "EXECUTE") => {
                Some(ExpectedObjectSuggestionKind::Routine)
            }
            [.., last] if matches!(last.as_str(), "DESC" | "DESCRIBE") => {
                Some(ExpectedObjectSuggestionKind::Any)
            }
            [.., prev, last]
                if matches!(
                    prev.as_str(),
                    "ALTER" | "DROP" | "TRUNCATE" | "FLASHBACK" | "LOCK"
                ) && *last == "TABLE" =>
            {
                Some(ExpectedObjectSuggestionKind::Table)
            }
            [.., a, b, c] if *a == "COMMENT" && *b == "ON" && *c == "TABLE" => {
                Some(ExpectedObjectSuggestionKind::Table)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "VIEW" => {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c] if *a == "COMMENT" && *b == "ON" && *c == "VIEW" => {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c, d]
                if *a == "COMMENT" && *b == "ON" && *c == "EDITIONING" && *d == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::View)
            }
            [.., a, b, c]
                if matches!(a.as_str(), "ALTER" | "DROP")
                    && *b == "MATERIALIZED"
                    && *c == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::MaterializedView)
            }
            [.., a, b, c, d]
                if *a == "COMMENT" && *b == "ON" && *c == "MATERIALIZED" && *d == "VIEW" =>
            {
                Some(ExpectedObjectSuggestionKind::MaterializedView)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "TYPE" => {
                Some(ExpectedObjectSuggestionKind::Type)
            }
            [.., a, b, c] if *a == "DROP" && *b == "TYPE" && *c == "BODY" => {
                Some(ExpectedObjectSuggestionKind::Type)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "TRIGGER" =>
            {
                Some(ExpectedObjectSuggestionKind::Trigger)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "EVENT" => {
                Some(ExpectedObjectSuggestionKind::Event)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "INDEX" => {
                Some(ExpectedObjectSuggestionKind::Index)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "PROCEDURE" =>
            {
                Some(ExpectedObjectSuggestionKind::Procedure)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "FUNCTION" =>
            {
                Some(ExpectedObjectSuggestionKind::Function)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "PACKAGE" => {
                Some(ExpectedObjectSuggestionKind::Package)
            }
            [.., a, b, c] if *a == "DROP" && *b == "PACKAGE" && *c == "BODY" => {
                Some(ExpectedObjectSuggestionKind::Package)
            }
            [.., prev, last]
                if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "SEQUENCE" =>
            {
                Some(ExpectedObjectSuggestionKind::Sequence)
            }
            [.., prev, last] if *prev == "DROP" && *last == "SYNONYM" => {
                Some(ExpectedObjectSuggestionKind::Synonym)
            }
            [.., a, b, c] if *a == "DROP" && *b == "PUBLIC" && *c == "SYNONYM" => {
                Some(ExpectedObjectSuggestionKind::PublicSynonym)
            }
            [.., prev, last] if matches!(prev.as_str(), "ALTER" | "DROP") && *last == "USER" => {
                Some(ExpectedObjectSuggestionKind::User)
            }
            [.., a, b, c, d]
                if *a == "ALTER" && *b == "SESSION" && *c == "SET" && *d == "CURRENT_SCHEMA" =>
            {
                Some(ExpectedObjectSuggestionKind::User)
            }
            _ => None,
        }
    }

    fn expected_grant_revoke_object_suggestion_kind(
        words: &[String],
    ) -> Option<ExpectedObjectSuggestionKind> {
        if words.last().is_none_or(|word| word != "ON") {
            return None;
        }
        let grant_idx = words
            .iter()
            .rposition(|word| matches!(word.as_str(), "GRANT" | "REVOKE"))?;
        let privilege_words = words.get(grant_idx + 1..words.len().saturating_sub(1))?;
        if privilege_words.is_empty() {
            return None;
        }
        if privilege_words
            .iter()
            .all(|word| Self::is_executable_object_privilege(word))
        {
            return Some(ExpectedObjectSuggestionKind::Executable);
        }
        if privilege_words
            .iter()
            .all(|word| Self::is_relation_or_sequence_object_privilege(word))
        {
            return Some(ExpectedObjectSuggestionKind::RelationOrSequence);
        }

        None
    }

    fn is_executable_object_privilege(word: &str) -> bool {
        matches!(word, "EXECUTE" | "DEBUG")
    }

    fn is_relation_or_sequence_object_privilege(word: &str) -> bool {
        matches!(
            word,
            "SELECT"
                | "READ"
                | "INSERT"
                | "UPDATE"
                | "DELETE"
                | "REFERENCES"
                | "INDEX"
                | "ALTER"
                | "FLASHBACK"
                | "QUERY"
                | "REWRITE"
        )
    }

    fn expected_suggestion_context_end(
        tokens: &[SqlToken],
        cursor_token_len: usize,
        exclude_current_identifier_chain: bool,
    ) -> usize {
        if !exclude_current_identifier_chain {
            return cursor_token_len.min(tokens.len());
        }

        Self::current_qualified_identifier_chain_start(tokens, cursor_token_len)
            .unwrap_or(cursor_token_len)
            .min(tokens.len())
    }

    fn collect_expected_object_suggestions_for_kind(
        data: &mut IntellisenseData,
        prefix: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> Vec<String> {
        let suggestions = match kind {
            ExpectedObjectSuggestionKind::Any => data.get_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Routine => data.get_routine_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Executable => data.get_executable_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::RelationOrSequence => {
                data.get_relation_or_sequence_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Table => data.get_table_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::View => data.get_view_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::MaterializedView => {
                data.get_materialized_view_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Type => data.get_type_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Trigger => data.get_trigger_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Event => data.get_event_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Index => data.get_index_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Procedure => {
                data.get_procedure_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::Function => data.get_function_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Package => data.get_package_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Sequence => data.get_sequence_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::Synonym => data.get_synonym_object_suggestions(prefix),
            ExpectedObjectSuggestionKind::PublicSynonym => {
                data.get_public_synonym_object_suggestions(prefix)
            }
            ExpectedObjectSuggestionKind::User => data.get_user_suggestions(prefix),
        };

        if prefix.is_empty() || matches!(kind, ExpectedObjectSuggestionKind::User) {
            return suggestions;
        }

        Self::merge_suggestions_with_context_aliases(
            suggestions,
            data.get_user_suggestions(prefix),
            false,
        )
    }

    fn matches_string_list_case_insensitive(values: &[String], candidate: &str) -> bool {
        values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(candidate))
    }

    fn suggestion_matches_expected_object_kind(
        data: &IntellisenseData,
        candidate: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> bool {
        match kind {
            ExpectedObjectSuggestionKind::Any => true,
            ExpectedObjectSuggestionKind::Routine => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
                    || Self::matches_string_list_case_insensitive(&data.functions, candidate)
                    || Self::matches_string_list_case_insensitive(&data.packages, candidate)
            }
            ExpectedObjectSuggestionKind::Executable => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
                    || Self::matches_string_list_case_insensitive(&data.functions, candidate)
                    || Self::matches_string_list_case_insensitive(&data.packages, candidate)
                    || Self::matches_string_list_case_insensitive(&data.types, candidate)
            }
            ExpectedObjectSuggestionKind::RelationOrSequence => {
                Self::matches_string_list_case_insensitive(&data.tables, candidate)
                    || Self::matches_string_list_case_insensitive(&data.views, candidate)
                    || Self::matches_string_list_case_insensitive(&data.materialized_views, candidate)
                    || Self::matches_string_list_case_insensitive(&data.sequences, candidate)
                    || Self::matches_string_list_case_insensitive(&data.synonyms, candidate)
                    || Self::matches_string_list_case_insensitive(&data.public_synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::Table => {
                Self::matches_string_list_case_insensitive(&data.tables, candidate)
            }
            ExpectedObjectSuggestionKind::View => {
                Self::matches_string_list_case_insensitive(&data.views, candidate)
            }
            ExpectedObjectSuggestionKind::MaterializedView => {
                Self::matches_string_list_case_insensitive(&data.materialized_views, candidate)
            }
            ExpectedObjectSuggestionKind::Type => {
                Self::matches_string_list_case_insensitive(&data.types, candidate)
            }
            ExpectedObjectSuggestionKind::Trigger => {
                Self::matches_string_list_case_insensitive(&data.triggers, candidate)
            }
            ExpectedObjectSuggestionKind::Event => {
                Self::matches_string_list_case_insensitive(&data.events, candidate)
            }
            ExpectedObjectSuggestionKind::Index => {
                Self::matches_string_list_case_insensitive(&data.indexes, candidate)
            }
            ExpectedObjectSuggestionKind::Procedure => {
                Self::matches_string_list_case_insensitive(&data.procedures, candidate)
            }
            ExpectedObjectSuggestionKind::Function => {
                Self::matches_string_list_case_insensitive(&data.functions, candidate)
            }
            ExpectedObjectSuggestionKind::Package => {
                Self::matches_string_list_case_insensitive(&data.packages, candidate)
            }
            ExpectedObjectSuggestionKind::Sequence => {
                Self::matches_string_list_case_insensitive(&data.sequences, candidate)
            }
            ExpectedObjectSuggestionKind::Synonym => {
                Self::matches_string_list_case_insensitive(&data.synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::PublicSynonym => {
                Self::matches_string_list_case_insensitive(&data.public_synonyms, candidate)
            }
            ExpectedObjectSuggestionKind::User => {
                Self::matches_string_list_case_insensitive(&data.users, candidate)
            }
        }
    }

    fn expected_qualifier_member_kinds(
        kind: ExpectedObjectSuggestionKind,
    ) -> Option<&'static [QualifiedMemberKind]> {
        match kind {
            ExpectedObjectSuggestionKind::Any => None,
            ExpectedObjectSuggestionKind::Routine => Some(&[
                QualifiedMemberKind::Procedure,
                QualifiedMemberKind::Function,
                QualifiedMemberKind::Package,
            ]),
            ExpectedObjectSuggestionKind::Executable => Some(&[
                QualifiedMemberKind::Procedure,
                QualifiedMemberKind::Function,
                QualifiedMemberKind::Package,
                QualifiedMemberKind::Type,
            ]),
            ExpectedObjectSuggestionKind::RelationOrSequence => Some(&[
                QualifiedMemberKind::Table,
                QualifiedMemberKind::View,
                QualifiedMemberKind::MaterializedView,
                QualifiedMemberKind::Sequence,
                QualifiedMemberKind::Synonym,
                QualifiedMemberKind::PublicSynonym,
            ]),
            ExpectedObjectSuggestionKind::Table => Some(&[QualifiedMemberKind::Table]),
            ExpectedObjectSuggestionKind::View => Some(&[QualifiedMemberKind::View]),
            ExpectedObjectSuggestionKind::MaterializedView => {
                Some(&[QualifiedMemberKind::MaterializedView])
            }
            ExpectedObjectSuggestionKind::Type => Some(&[QualifiedMemberKind::Type]),
            ExpectedObjectSuggestionKind::Trigger => Some(&[QualifiedMemberKind::Trigger]),
            ExpectedObjectSuggestionKind::Event => Some(&[QualifiedMemberKind::Event]),
            ExpectedObjectSuggestionKind::Index => Some(&[QualifiedMemberKind::Index]),
            ExpectedObjectSuggestionKind::Procedure => Some(&[QualifiedMemberKind::Procedure]),
            ExpectedObjectSuggestionKind::Function => Some(&[QualifiedMemberKind::Function]),
            ExpectedObjectSuggestionKind::Package => Some(&[QualifiedMemberKind::Package]),
            ExpectedObjectSuggestionKind::Sequence => Some(&[QualifiedMemberKind::Sequence]),
            ExpectedObjectSuggestionKind::Synonym => Some(&[QualifiedMemberKind::Synonym]),
            ExpectedObjectSuggestionKind::PublicSynonym => {
                Some(&[QualifiedMemberKind::PublicSynonym])
            }
            ExpectedObjectSuggestionKind::User => Some(&[QualifiedMemberKind::User]),
        }
    }

    fn suggestion_matches_expected_object_kind_for_qualifier(
        data: &IntellisenseData,
        qualifier: &str,
        candidate: &str,
        kind: ExpectedObjectSuggestionKind,
    ) -> bool {
        if let Some(expected_kinds) = Self::expected_qualifier_member_kinds(kind) {
            if let Some(matches) =
                data.qualifier_member_matches_kinds(qualifier, candidate, expected_kinds)
            {
                return matches;
            }
        }

        Self::suggestion_matches_expected_object_kind(data, candidate, kind)
    }

    fn expected_member_suggestions_for_qualifier(
        data: &mut IntellisenseData,
        qualifier: &str,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let suggestions = data.get_member_suggestions(qualifier, prefix, false);
        let Some(kind) = Self::expected_object_suggestion_kind(prefix, Some(qualifier), deep_ctx)
        else {
            return suggestions;
        };
        if matches!(kind, ExpectedObjectSuggestionKind::Any) {
            return suggestions;
        }

        let mut filtered = Vec::new();
        let mut seen = HashSet::new();
        for suggestion in suggestions {
            if !Self::suggestion_matches_expected_object_kind_for_qualifier(
                data,
                qualifier,
                &suggestion,
                kind,
            ) {
                continue;
            }
            if seen.insert(Self::completion_identifier_lookup_upper(&suggestion)) {
                filtered.push(suggestion);
            }
            if filtered.len() >= MAX_MERGED_SUGGESTIONS {
                break;
            }
        }
        filtered
    }

    fn expected_relation_member_suggestions_for_qualifier(
        data: &mut IntellisenseData,
        qualifier: &str,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let suggestions = data.get_member_suggestions(qualifier, prefix, true);
        let Some(kind) = Self::expected_object_suggestion_kind(prefix, Some(qualifier), deep_ctx)
        else {
            return suggestions;
        };
        let Some(expected_kinds) = Self::expected_qualifier_member_kinds(kind) else {
            return suggestions;
        };

        let mut filtered = Vec::new();
        let mut saw_kind_metadata = false;
        let mut seen = HashSet::new();
        for suggestion in &suggestions {
            let Some(matches) =
                data.qualifier_member_matches_kinds(qualifier, suggestion, expected_kinds)
            else {
                continue;
            };
            saw_kind_metadata = true;
            if matches && seen.insert(Self::completion_identifier_lookup_upper(suggestion)) {
                filtered.push(suggestion.clone());
            }
            if filtered.len() >= MAX_MERGED_SUGGESTIONS {
                break;
            }
        }

        if saw_kind_metadata {
            filtered
        } else {
            suggestions
        }
    }

    fn collect_expected_object_suggestions(
        data: &mut IntellisenseData,
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        match Self::expected_object_suggestion_kind(prefix, None, deep_ctx) {
            Some(kind) => Self::collect_expected_object_suggestions_for_kind(data, prefix, kind),
            None => Vec::new(),
        }
    }

    fn expand_virtual_table_wildcards(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        let wildcard_tables = intellisense_context::extract_select_list_wildcard_scopes(
            body_tokens,
            body_tables_in_scope,
        );
        if wildcard_tables.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let wildcard_tables = Self::effective_wildcard_column_tables(
            wildcard_tables,
            body_tables_in_scope,
            virtual_table_columns,
        );
        let mut wildcard_columns = Vec::new();
        for table in &wildcard_tables {
            if Self::virtual_table_columns_for_lookup(virtual_table_columns, table).is_none() {
                Self::request_table_columns(table, intellisense_data, column_sender, connection);
            }
            let columns = Self::columns_for_virtual_or_cached_table(
                table,
                virtual_table_columns,
                intellisense_data,
            );
            wildcard_columns.extend(columns);
        }
        Self::dedup_column_names_case_insensitive(&mut wildcard_columns);
        (wildcard_columns, wildcard_tables)
    }

    fn effective_wildcard_column_tables(
        wildcard_tables: Vec<String>,
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        wildcard_tables
            .into_iter()
            .map(|table| {
                if Self::virtual_table_columns_for_lookup(virtual_table_columns, &table).is_some() {
                    return table;
                }
                if let Some(source) = body_tables_in_scope.iter().find_map(|table_ref| {
                    table_ref
                        .alias
                        .as_deref()
                        .filter(|alias| alias.eq_ignore_ascii_case(&table))
                        .map(|_| table_ref.name.clone())
                }) {
                    return source;
                }
                body_tables_in_scope
                    .iter()
                    .find_map(|table_ref| {
                        let alias = table_ref.alias.as_deref()?;
                        (table_ref.name.eq_ignore_ascii_case(&table)
                            && Self::virtual_table_columns_for_lookup(virtual_table_columns, alias)
                                .is_some())
                        .then(|| alias.to_string())
                    })
                    .unwrap_or(table)
            })
            .collect()
    }

    fn columns_for_virtual_or_cached_table(
        table: &str,
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
    ) -> Vec<String> {
        if let Some(columns) = Self::virtual_table_columns_for_lookup(virtual_table_columns, table) {
            return columns.to_vec();
        }

        let data = intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.get_columns_for_table(table)
    }

    fn dedup_column_names_case_insensitive(columns: &mut Vec<String>) {
        let mut seen = HashSet::new();
        columns.retain(|column| seen.insert(Self::completion_identifier_lookup_upper(column)));
    }

    fn column_sets_match_case_insensitive(left: &[String], right: &[String]) -> bool {
        if left.len() != right.len() {
            return false;
        }

        let mut left_keys: Vec<String> = left
            .iter()
            .map(|column| Self::completion_identifier_lookup_upper(column))
            .collect();
        let mut right_keys: Vec<String> = right
            .iter()
            .map(|column| Self::completion_identifier_lookup_upper(column))
            .collect();
        left_keys.sort_unstable();
        right_keys.sort_unstable();
        left_keys == right_keys
    }

    fn dedup_local_member_entries_case_insensitive(entries: &mut Vec<LocalMemberEntry>) {
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert(entry.upper.clone()));
    }

    /// Column qualifier to use in generated SQL: the alias if present,
    /// otherwise the table's unqualified (short) name.
    fn auto_join_qualifier(table: &intellisense_context::ScopedTableRef) -> String {
        if let Some(alias) = table.alias.as_deref() {
            if !alias.is_empty() {
                return alias.to_string();
            }
        }

        Self::auto_join_relation_segments(&table.name)
            .and_then(|segments| {
                segments.last().map(|segment| {
                    if segment.quoted_dotted {
                        Self::quote_identifier_segment_for_completion(&segment.text)
                    } else {
                        segment.text.clone()
                    }
                })
            })
            .unwrap_or_else(|| Self::strip_identifier_quotes(&table.name))
    }

    /// Compare two table references by their unquoted short (unqualified) name.
    fn auto_join_tables_match(a: &str, b: &str) -> bool {
        let Some(a_key) = Self::auto_join_table_match_key(a) else {
            return false;
        };
        let Some(b_key) = Self::auto_join_table_match_key(b) else {
            return false;
        };

        if a_key.full == b_key.full {
            return true;
        }

        a_key.allow_short_match && b_key.allow_short_match && a_key.short == b_key.short
    }

    fn auto_join_table_match_key(value: &str) -> Option<AutoJoinTableMatchKey> {
        let segments = Self::auto_join_relation_segments(value)?;
        let last = segments.last()?;
        let full = segments
            .iter()
            .map(Self::auto_join_relation_segment_match_key)
            .collect::<Vec<_>>()
            .join(".");
        let short = Self::auto_join_relation_segment_match_key(last);

        Some(AutoJoinTableMatchKey {
            full,
            short,
            allow_short_match: !last.quoted_dotted,
        })
    }

    fn auto_join_relation_segment_match_key(segment: &AutoJoinRelationSegment) -> String {
        let kind = if segment.quoted_dotted { "Q" } else { "U" };
        format!("{kind}:{}", segment.text.to_ascii_uppercase())
    }

    fn auto_join_relation_segments(value: &str) -> Option<Vec<AutoJoinRelationSegment>> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;

        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
                    current.push(ch);
                    if active_quote == Some(ch) {
                        if chars.peek().copied() == Some(ch) {
                            current.push(ch);
                            chars.next();
                        } else {
                            active_quote = None;
                        }
                    } else if active_quote.is_none() {
                        active_quote = Some(ch);
                    }
                }
                '.' if active_quote.is_none() => {
                    segments.push(Self::auto_join_relation_segment(current.trim())?);
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        if active_quote.is_some() {
            return None;
        }

        segments.push(Self::auto_join_relation_segment(current.trim())?);
        Some(segments)
    }

    fn auto_join_relation_segment(raw: &str) -> Option<AutoJoinRelationSegment> {
        if raw.trim().is_empty() {
            return None;
        }

        let text = Self::strip_identifier_quotes(raw);
        if text.trim().is_empty() {
            return None;
        }

        Some(AutoJoinRelationSegment {
            quoted_dotted: sql_text::is_quoted_identifier(raw) && text.contains('.'),
            text,
        })
    }

    fn format_auto_join_pairs(
        left_q: &str,
        left_cols: &[String],
        right_q: &str,
        right_cols: &[String],
    ) -> String {
        let left_scope = Self::render_select_list_wildcard_scope(left_q);
        let right_scope = Self::render_select_list_wildcard_scope(right_q);
        left_cols
            .iter()
            .zip(right_cols)
            .map(|(left, right)| {
                format!(
                    "{}.{} = {}.{}",
                    left_scope,
                    Self::quote_identifier_segment_for_completion(left),
                    right_scope,
                    Self::quote_identifier_segment_for_completion(right)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Build an FK-based join condition (e.g. `e.DEPTNO = d.DEPTNO`) between the
    /// most-recently joined table (`right`) and an earlier table in scope, when
    /// a foreign key relates them in either direction. Returns the condition
    /// expression without the `ON` keyword. Prefers the nearest preceding table.
    fn build_auto_join_condition(
        data: &IntellisenseData,
        right: &intellisense_context::ScopedTableRef,
        lefts: &[&intellisense_context::ScopedTableRef],
    ) -> Option<String> {
        let right_q = Self::auto_join_qualifier(right);
        for left in lefts.iter().rev() {
            let left_q = Self::auto_join_qualifier(left);

            if let Some(fks) = data.get_foreign_keys(&right.name) {
                for fk in fks {
                    if Self::auto_join_tables_match(&fk.ref_table, &left.name)
                        && !fk.columns.is_empty()
                        && fk.columns.len() == fk.ref_columns.len()
                    {
                        return Some(Self::format_auto_join_pairs(
                            &right_q,
                            &fk.columns,
                            &left_q,
                            &fk.ref_columns,
                        ));
                    }
                }
            }

            if let Some(fks) = data.get_foreign_keys(&left.name) {
                for fk in fks {
                    if Self::auto_join_tables_match(&fk.ref_table, &right.name)
                        && !fk.columns.is_empty()
                        && fk.columns.len() == fk.ref_columns.len()
                    {
                        return Some(Self::format_auto_join_pairs(
                            &left_q,
                            &fk.columns,
                            &right_q,
                            &fk.ref_columns,
                        ));
                    }
                }
            }
        }
        None
    }

    /// Build the display-detail map (column name upper -> "TYPE  PK/NN  FK→T")
    /// for every column of the in-scope tables. First occurrence of a column
    /// name wins, matching how column suggestions are de-duplicated.
    fn build_column_descriptions(
        data: &IntellisenseData,
        column_tables: &[String],
    ) -> HashMap<String, String> {
        let mut descriptions: HashMap<String, String> = HashMap::new();
        for table in column_tables {
            let mut fk_targets: HashMap<String, String> = HashMap::new();
            if let Some(fks) = data.get_foreign_keys(table) {
                for fk in fks {
                    for column in &fk.columns {
                        fk_targets
                            .entry(Self::completion_identifier_lookup_upper(column))
                            .or_insert_with(|| fk.ref_table.clone());
                    }
                }
            }

            for column in data.get_columns_for_table(table) {
                let key = Self::completion_identifier_lookup_upper(&column);
                if descriptions.contains_key(&key) {
                    continue;
                }
                let Some(meta) = data.get_column_meta(table, &column) else {
                    continue;
                };

                let mut detail = meta.type_display.clone();
                if meta.is_primary_key {
                    detail.push_str("  PK");
                } else if !meta.nullable {
                    detail.push_str("  NN");
                }
                if let Some(ref_table) = fk_targets.get(&key) {
                    detail.push_str(&format!("  FK\u{2192}{ref_table}"));
                }

                if !detail.trim().is_empty() {
                    descriptions.insert(key, detail);
                }
            }
        }
        descriptions
    }

    fn has_column_loading_for_scope(
        include_columns: bool,
        column_tables: &[String],
        virtual_wildcard_dependencies: &HashMap<String, Vec<String>>,
        data: &IntellisenseData,
    ) -> bool {
        if !include_columns {
            return false;
        }

        fn table_is_loading(data: &IntellisenseData, table: &str) -> bool {
            if let Some(key) = SqlEditorWidget::resolve_table_column_load_key(data, table) {
                if data.columns_loading.contains(&key.to_uppercase()) {
                    return true;
                }
            }

            let upper = table.to_uppercase();
            if data.columns_loading.contains(&upper) {
                return true;
            }
            // Only build full candidate list when the name has a qualified dot.
            if !SqlEditorWidget::has_unquoted_dot(table) {
                return false;
            }
            SqlEditorWidget::table_lookup_key_candidates(table)
                .iter()
                .any(|key| {
                    let key_upper = key.to_uppercase();
                    key_upper != upper && data.columns_loading.contains(&key_upper)
                })
        }

        column_tables.iter().any(|table| {
            if table_is_loading(data, table) {
                return true;
            }
            let key = table.to_uppercase();
            virtual_wildcard_dependencies
                .get(&key)
                .is_some_and(|deps| deps.iter().any(|dep| table_is_loading(data, dep)))
        })
    }

    fn collect_context_name_suggestions(
        prefix: &str,
        deep_ctx: &intellisense_context::CursorContext,
        context: SqlContext,
    ) -> Vec<String> {
        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        let allow_relation_aliases = !matches!(context, SqlContext::TableName);

        let mut push_candidate = |candidate: &str| {
            if candidate.is_empty() {
                return;
            }
            let candidate_upper = Self::completion_identifier_lookup_upper(candidate);
            if !prefix_upper.is_empty() && !candidate_upper.starts_with(&prefix_upper) {
                return;
            }
            if seen.insert(candidate_upper) {
                suggestions.push(candidate.to_string());
            }
        };

        if allow_relation_aliases {
            for table_ref in &deep_ctx.tables_in_scope {
                if let Some(alias) = table_ref.alias.as_deref() {
                    push_candidate(alias);
                }
            }
        }

        for cte in &deep_ctx.ctes {
            push_candidate(&cte.name);
        }

        if allow_relation_aliases {
            for subq in &deep_ctx.subqueries {
                push_candidate(&subq.alias);
            }
        }

        suggestions
    }

    fn collect_clause_wildcard_suggestions(
        prefix: &str,
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let policy = ClauseCompletionPolicy::for_phase(deep_ctx.phase, qualifier.is_some());
        let prefix_upper = Self::completion_wildcard_candidate_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();

        let mut push_candidate = |candidate: String| {
            if candidate.is_empty() {
                return;
            }
            let candidate_upper = Self::completion_wildcard_candidate_lookup_upper(&candidate);
            if !prefix_upper.is_empty() && !candidate_upper.starts_with(prefix_upper.as_str()) {
                return;
            }
            if seen.insert(candidate_upper) {
                suggestions.push(candidate);
            }
        };

        match policy.select_list_wildcard_mode {
            SelectListWildcardMode::None => {}
            SelectListWildcardMode::Qualified => {
                push_candidate("*".to_string());
            }
            SelectListWildcardMode::Unqualified => {
                push_candidate("*".to_string());
                let current_query_tokens = Self::current_query_tokens(deep_ctx);
                let current_query_tables =
                    intellisense_context::collect_tables_in_statement(current_query_tokens);
                for table_ref in current_query_tables {
                    let scope_name = table_ref
                        .alias
                        .as_deref()
                        .unwrap_or(table_ref.name.as_str());
                    let rendered_scope = Self::render_select_list_wildcard_scope(scope_name);
                    if !rendered_scope.is_empty() {
                        push_candidate(format!("{rendered_scope}.*"));
                    }
                }
            }
        }

        suggestions.truncate(MAX_MERGED_SUGGESTIONS);
        suggestions
    }

    fn merge_suggestions_with_context_aliases(
        mut base: Vec<String>,
        aliases: Vec<String>,
        prefer_aliases: bool,
    ) -> Vec<String> {
        if aliases.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let mut seen: HashSet<String> = base
            .iter()
            .map(|item| Self::completion_identifier_lookup_upper(item))
            .collect();
        let mut filtered_aliases = Vec::new();
        for alias in aliases {
            if seen.insert(Self::completion_identifier_lookup_upper(&alias)) {
                filtered_aliases.push(alias);
            }
        }

        if filtered_aliases.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let mut merged = if prefer_aliases {
            filtered_aliases.extend(base);
            filtered_aliases
        } else {
            base.extend(filtered_aliases);
            base
        };
        merged.truncate(MAX_MERGED_SUGGESTIONS);
        merged
    }

    fn qualified_condition_comparison_precedes_base_suggestions(
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        matches!(phase, intellisense_context::SqlPhase::JoinCondition)
    }

    fn merge_qualified_condition_comparison_suggestions(
        base: Vec<String>,
        comparisons: Vec<String>,
        phase: intellisense_context::SqlPhase,
    ) -> Vec<String> {
        Self::merge_suggestions_with_context_aliases(
            base,
            comparisons,
            Self::qualified_condition_comparison_precedes_base_suggestions(phase),
        )
    }

    fn maybe_merge_suggestions_with_context_aliases(
        mut base: Vec<String>,
        aliases: Vec<String>,
        prefer_aliases: bool,
        has_qualifier: bool,
    ) -> Vec<String> {
        if has_qualifier {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }
        Self::merge_suggestions_with_context_aliases(base, aliases, prefer_aliases)
    }

    fn infer_columns_from_partial_select_qualifiers(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> Vec<String> {
        let qualifiers = intellisense_context::extract_select_list_leading_qualifiers(body_tokens);
        if qualifiers.is_empty() {
            return Vec::new();
        }

        let mut columns = Vec::new();
        for qualifier in qualifiers {
            let mut tables =
                intellisense_context::resolve_qualifier_tables(&qualifier, body_tables_in_scope);
            let unresolved_direct =
                tables.len() == 1 && tables[0].eq_ignore_ascii_case(qualifier.as_str());
            if unresolved_direct {
                let outer_tables = intellisense_context::resolve_qualifier_tables(
                    &qualifier,
                    outer_tables_in_scope,
                );
                let outer_unresolved_direct = outer_tables.len() == 1
                    && outer_tables[0].eq_ignore_ascii_case(qualifier.as_str());
                if !outer_unresolved_direct {
                    tables = outer_tables;
                }
            }

            for table in tables {
                if let Some(virtual_cols) =
                    Self::virtual_table_columns_for_lookup(virtual_table_columns, &table)
                {
                    columns.extend(virtual_cols.iter().cloned());
                    continue;
                }

                let mut table_columns = {
                    let data = intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    data.get_columns_for_table(&table)
                };
                if table_columns.is_empty() {
                    Self::request_table_columns(
                        &table,
                        intellisense_data,
                        column_sender,
                        connection,
                    );
                    table_columns = {
                        let data = intellisense_data
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        data.get_columns_for_table(&table)
                    };
                }
                columns.extend(table_columns);
            }
        }

        Self::dedup_column_names_case_insensitive(&mut columns);
        columns
    }

    fn build_virtual_table_columns_for_query_body(
        body_tokens: &[SqlToken],
        seed_virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> HashMap<String, Vec<String>> {
        let body_ctx = intellisense_context::analyze_cursor_context(body_tokens, body_tokens.len());
        let mut virtual_table_columns = seed_virtual_table_columns.clone();

        for cte in &body_ctx.ctes {
            let (columns, _) = Self::collect_cte_virtual_columns_for_completion(
                &body_ctx,
                cte,
                &virtual_table_columns,
                intellisense_data,
                column_sender,
                connection,
            );
            if !columns.is_empty() {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &cte.name, columns);
            }
        }

        for subq in &body_ctx.subqueries {
            if let Some(columns) = Self::explicit_subquery_columns_for_completion(&body_ctx, subq) {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &subq.alias, columns);
                continue;
            }
            let relation_tokens = intellisense_context::token_range_slice(
                body_ctx.statement_tokens.as_ref(),
                subq.body_range,
            );
            let relation_ctx = intellisense_context::analyze_cursor_context(
                relation_tokens,
                relation_tokens.len(),
            );
            let mut relation_virtual_table_columns = virtual_table_columns.clone();

            for cte in &relation_ctx.ctes {
                let (columns, _) = Self::collect_cte_virtual_columns_for_completion(
                    &relation_ctx,
                    cte,
                    &relation_virtual_table_columns,
                    intellisense_data,
                    column_sender,
                    connection,
                );
                if !columns.is_empty() {
                    Self::insert_virtual_table_columns(
                        &mut relation_virtual_table_columns,
                        &cte.name,
                        columns,
                    );
                }
            }

            let relation_local_tables =
                intellisense_context::collect_tables_in_statement(relation_tokens);
            let (columns, _) = Self::collect_virtual_relation_columns_for_completion(
                relation_tokens,
                &relation_local_tables,
                &body_ctx.tables_in_scope,
                &relation_virtual_table_columns,
                intellisense_data,
                column_sender,
                connection,
            );
            if !columns.is_empty() {
                Self::insert_virtual_table_columns(&mut virtual_table_columns, &subq.alias, columns);
            }
        }

        virtual_table_columns
    }

    fn collect_virtual_query_projection_columns(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        let available_virtual_table_columns = Self::build_virtual_table_columns_for_query_body(
            body_tokens,
            virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        );
        let pivot_unpivot_columns =
            intellisense_context::extract_oracle_pivot_unpivot_projection_columns(body_tokens);
        let mut columns = intellisense_context::extract_select_list_columns(body_tokens);
        let mut use_pivot_unpivot_projection =
            columns.is_empty() && !pivot_unpivot_columns.is_empty();
        if !columns.is_empty() && !pivot_unpivot_columns.is_empty() {
            let pivot_unpivot_source_columns =
                intellisense_context::extract_oracle_pivot_unpivot_source_projection_columns(
                    body_tokens,
                );
            if Self::column_sets_match_case_insensitive(&columns, &pivot_unpivot_source_columns) {
                columns.clear();
                use_pivot_unpivot_projection = true;
            }
        }
        if columns.is_empty() {
            columns = intellisense_context::extract_table_function_columns(body_tokens);
        }
        columns.extend(Self::infer_columns_from_partial_select_qualifiers(
            body_tokens,
            body_tables_in_scope,
            outer_tables_in_scope,
            &available_virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        ));

        let (wildcard_columns, wildcard_tables) = Self::expand_virtual_table_wildcards(
            body_tokens,
            body_tables_in_scope,
            &available_virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        );
        columns.extend(wildcard_columns);
        if use_pivot_unpivot_projection {
            columns.extend(pivot_unpivot_columns);
        }
        columns.extend(intellisense_context::extract_oracle_model_generated_columns(body_tokens));
        columns
            .extend(intellisense_context::extract_match_recognize_generated_columns(body_tokens));
        Self::dedup_column_names_case_insensitive(&mut columns);
        (columns, wildcard_tables)
    }

    fn collect_virtual_relation_columns_for_completion(
        body_tokens: &[SqlToken],
        body_tables_in_scope: &[intellisense_context::ScopedTableRef],
        outer_tables_in_scope: &[intellisense_context::ScopedTableRef],
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        Self::collect_virtual_query_projection_columns(
            body_tokens,
            body_tables_in_scope,
            outer_tables_in_scope,
            virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
        )
    }

    fn collect_common_column_suggestions(
        prefix: &str,
        column_tables: &[String],
        data: &IntellisenseData,
    ) -> Vec<String> {
        if column_tables.len() < 2 {
            return Vec::new();
        }

        let mut iter = column_tables.iter();
        let Some(first_table) = iter.next() else {
            return Vec::new();
        };
        let mut common_columns: Vec<(String, String)> = data
            .get_columns_for_table(first_table)
            .into_iter()
            .map(|column| {
                let upper = Self::completion_identifier_lookup_upper(&column);
                (column, upper)
            })
            .collect();
        if common_columns.is_empty() {
            return Vec::new();
        }

        for table in iter {
            let table_columns = data.get_columns_for_table(table);
            if table_columns.is_empty() {
                return Vec::new();
            }
            let allowed: HashSet<String> = table_columns
                .into_iter()
                .map(|column| Self::completion_identifier_lookup_upper(&column))
                .collect();
            common_columns
                .retain(|(_, upper)| allowed.contains(upper));
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut suggestions = Vec::new();
        let mut seen = HashSet::new();
        for (column, upper) in common_columns {
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                suggestions.push(column);
                if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                    break;
                }
            }
        }
        suggestions
    }

    fn current_query_tokens(deep_ctx: &intellisense_context::CursorContext) -> &[SqlToken] {
        deep_ctx
            .active_query_range
            .map(|range| {
                intellisense_context::token_range_slice(deep_ctx.statement_tokens.as_ref(), range)
            })
            .unwrap_or_else(|| deep_ctx.statement_tokens.as_ref())
    }

    fn cursor_token_len_in_current_query(deep_ctx: &intellisense_context::CursorContext) -> usize {
        deep_ctx
            .active_query_range
            .map(|range| {
                deep_ctx
                    .cursor_token_len
                    .saturating_sub(range.start)
                    .min(range.end.saturating_sub(range.start))
            })
            .unwrap_or(deep_ctx.cursor_token_len)
    }

    fn next_word_upper_in_tokens(tokens: &[SqlToken], idx: usize) -> Option<(String, usize)> {
        let mut current_idx = idx;
        while current_idx < tokens.len() {
            match &tokens[current_idx] {
                SqlToken::Comment(_) => current_idx += 1,
                SqlToken::Word(word) => return Some((word.to_ascii_uppercase(), current_idx)),
                _ => return None,
            }
        }
        None
    }

    fn cursor_is_in_query_level_order_by(deep_ctx: &intellisense_context::CursorContext) -> bool {
        if !matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::OrderByClause
        ) {
            return false;
        }

        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let token_depths = crate::ui::sql_depth::paren_depths(current_query_tokens);
        let mut idx = 0usize;
        let limit = cursor_token_len.min(current_query_tokens.len());

        while idx < limit {
            if !crate::ui::sql_depth::is_top_level_depth(&token_depths, idx) {
                idx += 1;
                continue;
            }

            let SqlToken::Word(word) = &current_query_tokens[idx] else {
                idx += 1;
                continue;
            };
            if !word.eq_ignore_ascii_case("ORDER") {
                idx += 1;
                continue;
            }

            let Some((next_keyword, next_idx)) =
                Self::next_word_upper_in_tokens(current_query_tokens, idx + 1)
            else {
                return false;
            };

            if next_keyword == "BY" && next_idx < limit {
                return true;
            }

            if next_keyword == "SIBLINGS" {
                if let Some((tail_keyword, tail_idx)) =
                    Self::next_word_upper_in_tokens(current_query_tokens, next_idx + 1)
                {
                    if tail_keyword == "BY" && tail_idx < limit {
                        return true;
                    }
                }
            }

            idx += 1;
        }

        false
    }

    fn virtual_table_columns_for_lookup<'a>(
        virtual_table_columns: &'a HashMap<String, Vec<String>>,
        table: &str,
    ) -> Option<&'a [String]> {
        let candidates = Self::table_lookup_key_candidates(table);
        for candidate in &candidates {
            let key = Self::completion_identifier_lookup_upper(candidate);
            if let Some(columns) = virtual_table_columns.get(&key) {
                return Some(columns.as_slice());
            }
        }

        let key = Self::completion_identifier_lookup_upper(table);
        virtual_table_columns.get(&key).map(|cols| cols.as_slice())
    }

    fn insert_virtual_table_columns(
        virtual_table_columns: &mut HashMap<String, Vec<String>>,
        relation_name: &str,
        columns: Vec<String>,
    ) {
        virtual_table_columns.insert(Self::completion_identifier_lookup_upper(relation_name), columns);
    }

    fn is_cursor_inside_subquery_explicit_column_list(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> bool {
        let cursor_token_idx = deep_ctx
            .cursor_token_len
            .min(deep_ctx.statement_tokens.len());
        subquery
            .explicit_column_range
            .is_some_and(|range| cursor_token_idx >= range.start && cursor_token_idx <= range.end)
    }

    fn explicit_subquery_columns_for_completion(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> Option<Vec<String>> {
        if subquery.explicit_columns.is_empty()
            || Self::is_cursor_inside_subquery_explicit_column_list(deep_ctx, subquery)
        {
            return None;
        }

        let mut columns = subquery.explicit_columns.clone();
        Self::dedup_column_names_case_insensitive(&mut columns);
        (!columns.is_empty()).then_some(columns)
    }

    fn virtual_subquery_replaces_source_columns(
        deep_ctx: &intellisense_context::CursorContext,
        subquery: &intellisense_context::SubqueryDefinition,
    ) -> bool {
        if Self::explicit_subquery_columns_for_completion(deep_ctx, subquery).is_some() {
            return true;
        }

        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            subquery.body_range,
        );
        !intellisense_context::extract_oracle_pivot_unpivot_projection_columns(body_tokens)
            .is_empty()
    }

    fn column_lookup_table_for_table_ref(
        table_ref: &intellisense_context::ScopedTableRef,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> String {
        if let Some(alias) = table_ref.alias.as_deref() {
            if let Some(subquery) = deep_ctx
                .subqueries
                .iter()
                .find(|subquery| subquery.alias.eq_ignore_ascii_case(alias))
            {
                if Self::virtual_subquery_replaces_source_columns(deep_ctx, subquery) {
                    return subquery.alias.clone();
                }
            }
        }

        table_ref.name.clone()
    }

    fn resolve_all_scope_column_lookup_tables(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let mut ordered_tables: Vec<(usize, &intellisense_context::ScopedTableRef)> =
            deep_ctx.tables_in_scope.iter().enumerate().collect();
        ordered_tables.sort_by(|(left_idx, left), (right_idx, right)| {
            right
                .depth
                .cmp(&left.depth)
                .then_with(|| left_idx.cmp(right_idx))
        });

        let mut tables = Vec::new();
        let mut seen = HashSet::new();

        for (_, table_ref) in ordered_tables {
            let lookup_table = Self::column_lookup_table_for_table_ref(table_ref, deep_ctx);
            let key = Self::completion_identifier_lookup_upper(&lookup_table);
            if seen.insert(key) {
                tables.push(lookup_table);
            }
        }

        tables
    }

    fn resolve_focused_column_lookup_tables(
        focused_tables: &[String],
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let mut tables = Vec::new();
        let mut seen = HashSet::new();

        for focused in focused_tables {
            let mut matched = false;
            for table_ref in &deep_ctx.tables_in_scope {
                let matches_focused = table_ref.name.eq_ignore_ascii_case(focused)
                    || table_ref
                        .alias
                        .as_deref()
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(focused));
                if !matches_focused {
                    continue;
                }

                matched = true;
                let lookup_table = Self::column_lookup_table_for_table_ref(table_ref, deep_ctx);
                let key = Self::completion_identifier_lookup_upper(&lookup_table);
                if seen.insert(key) {
                    tables.push(lookup_table);
                }
            }

            if !matched {
                let key = Self::completion_identifier_lookup_upper(focused);
                if seen.insert(key) {
                    tables.push(focused.clone());
                }
            }
        }

        tables
    }

    fn resolve_column_tables_for_context(
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        fn virtual_alias_for_qualifier<'a>(
            qualifier: &str,
            deep_ctx: &'a intellisense_context::CursorContext,
        ) -> Option<&'a intellisense_context::SubqueryDefinition> {
            deep_ctx
                .subqueries
                .iter()
                .find(|subq| subq.alias.eq_ignore_ascii_case(qualifier))
        }

        fn replacement_virtual_alias_scope(
            qualifier: &str,
            deep_ctx: &intellisense_context::CursorContext,
        ) -> Option<Vec<String>> {
            let subquery = virtual_alias_for_qualifier(qualifier, deep_ctx)?;
            SqlEditorWidget::virtual_subquery_replaces_source_columns(deep_ctx, subquery)
                .then(|| vec![subquery.alias.clone()])
        }

        fn prepend_virtual_alias_if_present(
            tables: &mut Vec<String>,
            qualifier: &str,
            deep_ctx: &intellisense_context::CursorContext,
        ) {
            let Some(subquery) = virtual_alias_for_qualifier(qualifier, deep_ctx) else {
                return;
            };
            let alias = subquery.alias.clone();

            if tables
                .iter()
                .any(|table| table.eq_ignore_ascii_case(&alias))
            {
                if let Some(existing_idx) = tables
                    .iter()
                    .position(|table| table.eq_ignore_ascii_case(&alias))
                {
                    if existing_idx != 0 {
                        let existing = tables.remove(existing_idx);
                        tables.insert(0, existing);
                    }
                }
                return;
            }

            tables.insert(0, alias);
        }

        let focused_tables =
            (!deep_ctx.focused_tables.is_empty()).then_some(&deep_ctx.focused_tables);
        if qualifier.is_some()
            && matches!(
                deep_ctx.phase,
                intellisense_context::SqlPhase::JoinUsingColumnList
            )
        {
            return Vec::new();
        }
        let Some(qualifier) = qualifier else {
            if let Some(focused_tables) = focused_tables {
                if matches!(
                    deep_ctx.phase,
                    intellisense_context::SqlPhase::LockingColumnList
                ) {
                    return Self::resolve_focused_column_lookup_tables(focused_tables, deep_ctx);
                }
                return focused_tables.to_vec();
            }
            return Self::resolve_all_scope_column_lookup_tables(deep_ctx);
        };

        let resolved =
            intellisense_context::resolve_qualifier_tables(qualifier, &deep_ctx.tables_in_scope);
        if let Some(virtual_scope) = replacement_virtual_alias_scope(qualifier, deep_ctx) {
            return virtual_scope;
        }
        if let Some(focused_tables) = focused_tables {
            let filtered: Vec<String> = resolved
                .iter()
                .filter(|name| {
                    focused_tables
                        .iter()
                        .any(|focused| focused.eq_ignore_ascii_case(name))
                })
                .cloned()
                .collect();
            if !filtered.is_empty() {
                let mut filtered = filtered;
                prepend_virtual_alias_if_present(&mut filtered, qualifier, deep_ctx);
                return filtered;
            }
        }
        let unresolved_direct = resolved.len() == 1 && resolved[0].eq_ignore_ascii_case(qualifier);
        if !unresolved_direct {
            if focused_tables.is_some() {
                return Vec::new();
            }
            let mut resolved = resolved;
            prepend_virtual_alias_if_present(&mut resolved, qualifier, deep_ctx);
            return resolved;
        }

        let pattern_vars = intellisense_context::extract_match_recognize_pattern_variables(
            Self::current_query_tokens(deep_ctx),
        );
        if pattern_vars
            .iter()
            .any(|var| var.eq_ignore_ascii_case(qualifier))
        {
            return intellisense_context::resolve_all_scope_tables(&deep_ctx.tables_in_scope);
        }

        let mut resolved = resolved;
        prepend_virtual_alias_if_present(&mut resolved, qualifier, deep_ctx);
        resolved
    }

    fn token_is_qualified_identifier_segment(token: &SqlToken) -> bool {
        matches!(token, SqlToken::Word(_) | SqlToken::String(_))
    }

    fn current_qualified_identifier_chain_start(
        tokens: &[SqlToken],
        cursor_token_len: usize,
    ) -> Option<usize> {
        if cursor_token_len == 0 || cursor_token_len > tokens.len() {
            return None;
        }

        let mut start = cursor_token_len;
        if start >= 2
            && matches!(tokens.get(start - 1), Some(SqlToken::Symbol(symbol)) if symbol == ".")
            && tokens
                .get(start - 2)
                .is_some_and(Self::token_is_qualified_identifier_segment)
        {
            start -= 2;
        } else if tokens
            .get(start - 1)
            .is_some_and(Self::token_is_qualified_identifier_segment)
        {
            start -= 1;
            while start >= 2
                && matches!(tokens.get(start - 1), Some(SqlToken::Symbol(symbol)) if symbol == ".")
                && tokens
                    .get(start - 2)
                    .is_some_and(Self::token_is_qualified_identifier_segment)
            {
                start -= 2;
            }
        } else {
            return None;
        }

        Some(start)
    }

    fn previous_non_comment_token(tokens: &[SqlToken], end: usize) -> Option<&SqlToken> {
        tokens
            .get(..end)?
            .iter()
            .rev()
            .find(|token| !matches!(token, SqlToken::Comment(_)))
    }

    fn cursor_has_existing_equals_before_qualified_identifier(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> bool {
        let tokens = Self::current_query_tokens(deep_ctx);
        let cursor_token_len = Self::cursor_token_len_in_current_query(deep_ctx);
        let Some(chain_start) =
            Self::current_qualified_identifier_chain_start(tokens, cursor_token_len)
        else {
            return false;
        };

        matches!(
            Self::previous_non_comment_token(tokens, chain_start),
            Some(SqlToken::Symbol(symbol)) if symbol == "="
        )
    }

    fn supports_qualified_condition_comparison_suggestions(
        phase: intellisense_context::SqlPhase,
    ) -> bool {
        matches!(
            phase,
            intellisense_context::SqlPhase::JoinCondition
                | intellisense_context::SqlPhase::WhereClause
                | intellisense_context::SqlPhase::HavingClause
                | intellisense_context::SqlPhase::ConnectByClause
                | intellisense_context::SqlPhase::StartWithClause
                | intellisense_context::SqlPhase::MatchRecognizeClause
        )
    }

    fn current_query_tables_for_condition_completion(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<intellisense_context::ScopedTableRef> {
        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let current_query_tables = if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            intellisense_context::collect_tables_in_statement_declared_before_cursor(
                current_query_tokens,
                Self::cursor_token_len_in_current_query(deep_ctx),
            )
        } else {
            intellisense_context::collect_tables_in_statement(current_query_tokens)
        };
        if current_query_tables.is_empty() {
            deep_ctx.tables_in_scope.clone()
        } else {
            current_query_tables
        }
    }

    fn comparison_scope_tables_for_context(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<intellisense_context::ScopedTableRef> {
        let mut tables = Self::current_query_tables_for_condition_completion(deep_ctx);

        // In JOIN ON we deliberately exclude later-declared tables (see
        // `current_query_tables_for_condition_completion`). Skip the outer-scope
        // merge here so we don't re-introduce a table that lives in
        // `tables_in_scope` only because the full statement was parsed.
        if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            return tables;
        }

        for table in &deep_ctx.tables_in_scope {
            let already_present = tables.iter().any(|existing| {
                existing.depth == table.depth
                    && existing.is_cte == table.is_cte
                    && existing.name.eq_ignore_ascii_case(&table.name)
                    && match (&existing.alias, &table.alias) {
                        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
                        (None, None) => true,
                        _ => false,
                    }
            });
            if !already_present {
                tables.push(table.clone());
            }
        }

        tables
    }

    fn comparison_lookup_tables_for_context(
        qualifier: Option<&str>,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let Some(qualifier) = qualifier else {
            return Vec::new();
        };
        if !Self::supports_qualified_condition_comparison_suggestions(deep_ctx.phase) {
            return Vec::new();
        }
        if Self::cursor_has_existing_equals_before_qualified_identifier(deep_ctx) {
            return Vec::new();
        }

        let comparison_tables = Self::comparison_scope_tables_for_context(deep_ctx);
        if comparison_tables.is_empty() {
            return Vec::new();
        }

        let mut lookup_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        for table_ref in comparison_tables {
            let table_lookup =
                Self::comparison_column_lookup_table_for_table_ref(&table_ref, deep_ctx);
            if lookup_tables
                .iter()
                .all(|existing| !existing.eq_ignore_ascii_case(&table_lookup))
            {
                lookup_tables.push(table_lookup);
            }
        }
        lookup_tables
    }

    fn comparison_column_lookup_table_for_table_ref(
        table_ref: &intellisense_context::ScopedTableRef,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> String {
        Self::column_lookup_table_for_table_ref(table_ref, deep_ctx)
    }

    fn collect_qualified_condition_comparison_suggestions(
        data: &IntellisenseData,
        prefix: &str,
        qualifier: &str,
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        if !Self::supports_qualified_condition_comparison_suggestions(deep_ctx.phase) {
            return Vec::new();
        }
        if Self::cursor_has_existing_equals_before_qualified_identifier(deep_ctx) {
            return Vec::new();
        }

        let comparison_tables = Self::comparison_scope_tables_for_context(deep_ctx);
        if comparison_tables.is_empty() {
            return Vec::new();
        }

        let target_tables = Self::resolve_column_tables_for_context(Some(qualifier), deep_ctx);
        if target_tables.is_empty() {
            return Vec::new();
        }

        let left_scope = Self::render_select_list_wildcard_scope(qualifier);
        if left_scope.is_empty() {
            return Vec::new();
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut target_columns = Vec::new();
        let mut seen_target_columns = HashSet::new();
        for table in &target_tables {
            for column in data.get_columns_for_table(table) {
                let upper = Self::completion_identifier_lookup_upper(&column);
                if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                    continue;
                }
                if seen_target_columns.insert(upper.clone()) {
                    target_columns.push((upper, column));
                }
            }
        }
        if target_columns.is_empty() {
            return Vec::new();
        }

        let pattern_variables = matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::MatchRecognizeClause
        )
        .then(|| intellisense_context::extract_match_recognize_pattern_variables(
            Self::current_query_tokens(deep_ctx),
        ))
        .filter(|variables| {
            variables
                .iter()
                .any(|variable| variable.eq_ignore_ascii_case(qualifier))
        });
        if let Some(pattern_variables) = pattern_variables {
            let mut suggestions = Vec::new();
            let mut seen_suggestions = HashSet::new();

            for other_pattern in pattern_variables {
                if Self::completion_identifiers_match(&other_pattern, qualifier) {
                    continue;
                }

                let rendered_other_scope =
                    Self::render_select_list_wildcard_scope(other_pattern.as_str());
                if rendered_other_scope.is_empty() {
                    continue;
                }

                for (_, target_column) in &target_columns {
                    let suggestion = format!(
                        "{}.{} = {}.{}",
                        left_scope,
                        Self::quote_identifier_segment_for_completion(target_column),
                        rendered_other_scope,
                        Self::quote_identifier_segment_for_completion(target_column),
                    );
                    if seen_suggestions.insert(suggestion.to_ascii_uppercase()) {
                        suggestions.push(suggestion);
                        if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                            return suggestions;
                        }
                    }
                }
            }

            return suggestions;
        }

        // In a JOIN ON clause the comparison is most naturally written against the
        // table being joined, i.e. the most recently declared one (e.g. `c` in
        // `JOIN ... c ON b.|`). Offer comparisons against later-declared tables first
        // so the current join target is suggested before earlier tables.
        let ordered_comparison_tables: Vec<&intellisense_context::ScopedTableRef> = if matches!(
            deep_ctx.phase,
            intellisense_context::SqlPhase::JoinCondition
        ) {
            comparison_tables.iter().rev().collect()
        } else {
            comparison_tables.iter().collect()
        };

        let mut suggestions = Vec::new();
        let mut seen_suggestions = HashSet::new();
        for table_ref in ordered_comparison_tables {
            let other_scope_name = table_ref
                .alias
                .as_deref()
                .unwrap_or(table_ref.name.as_str());
            if Self::completion_identifiers_match(other_scope_name, qualifier) {
                continue;
            }

            let rendered_other_scope = Self::render_select_list_wildcard_scope(other_scope_name);
            if rendered_other_scope.is_empty() {
                continue;
            }

            let mut other_columns_by_upper = HashMap::new();
            let other_lookup_table =
                Self::comparison_column_lookup_table_for_table_ref(table_ref, deep_ctx);
            for column in data.get_columns_for_table(&other_lookup_table) {
                other_columns_by_upper
                    .entry(Self::completion_identifier_lookup_upper(&column))
                    .or_insert(column);
            }
            if other_columns_by_upper.is_empty() {
                continue;
            }

            for (upper, target_column) in &target_columns {
                let Some(other_column) = other_columns_by_upper.get(upper) else {
                    continue;
                };
                let suggestion = format!(
                    "{}.{} = {}.{}",
                    left_scope,
                    Self::quote_identifier_segment_for_completion(target_column),
                    rendered_other_scope,
                    Self::quote_identifier_segment_for_completion(other_column),
                );
                if seen_suggestions.insert(suggestion.to_ascii_uppercase()) {
                    suggestions.push(suggestion);
                    if suggestions.len() >= MAX_MERGED_SUGGESTIONS {
                        return suggestions;
                    }
                }
            }
        }

        suggestions
    }

    fn merge_suggestions_with_derived_columns(
        mut base: Vec<String>,
        prefix: &str,
        derived_columns: Vec<String>,
    ) -> Vec<String> {
        if derived_columns.is_empty() {
            base.truncate(MAX_MERGED_SUGGESTIONS);
            return base;
        }

        let prefix_upper = Self::completion_identifier_lookup_upper(prefix);
        let mut seen: HashSet<String> = base
            .iter()
            .map(|item| Self::completion_identifier_lookup_upper(item))
            .collect();

        for column in derived_columns {
            let upper = Self::completion_identifier_lookup_upper(&column);
            if !prefix_upper.is_empty() && !upper.starts_with(prefix_upper.as_str()) {
                continue;
            }
            if seen.insert(upper) {
                base.push(column);
            }
        }

        base.truncate(MAX_MERGED_SUGGESTIONS);
        base
    }

    fn collect_derived_columns_for_context(
        deep_ctx: &intellisense_context::CursorContext,
    ) -> Vec<String> {
        let current_query_tokens = Self::current_query_tokens(deep_ctx);
        let mut derived_columns =
            intellisense_context::extract_oracle_unpivot_generated_columns(current_query_tokens);
        derived_columns.extend(
            intellisense_context::extract_oracle_model_generated_columns(current_query_tokens),
        );
        derived_columns.extend(
            intellisense_context::extract_match_recognize_generated_columns(current_query_tokens),
        );

        if Self::cursor_is_in_query_level_order_by(deep_ctx) {
            derived_columns.extend(intellisense_context::extract_select_list_columns(
                current_query_tokens,
            ));
        }

        Self::dedup_column_names_case_insensitive(&mut derived_columns);
        derived_columns
    }

    fn maybe_prefetch_columns_for_word(
        word: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        if word.is_empty() {
            return;
        }

        let should_prefetch = {
            let data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::resolve_table_column_load_key(&data, word).is_some()
        };

        if should_prefetch {
            Self::request_table_columns(word, intellisense_data, column_sender, connection);
        }
    }

    fn request_table_columns(
        table_name: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        let table_key = {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(selected) = Self::resolve_table_column_load_key(&data, table_name) else {
                return;
            };
            if !data.mark_columns_loading(&selected) {
                return;
            }
            selected
        };

        let task = ColumnLoadTask {
            table_key,
            connection: connection.clone(),
            sender: column_sender.clone(),
            foreign_keys: false,
        };

        if let Err(task) = Self::enqueue_column_load_task(task) {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                &format!(
                    "failed to enqueue column loader task for {}",
                    task.table_key
                ),
            );
            let _ = task.sender.send(ColumnLoadUpdate {
                table: task.table_key,
                columns: Vec::new(),
                column_meta: HashMap::new(),
                foreign_keys: Vec::new(),
                is_foreign_keys: false,
                cache_columns: false,
            });
            app::awake();
        }
    }

    /// Enqueue a lazy foreign-key load for `table_name` (used only when filling
    /// a JOIN ... ON clause), deduplicated against in-flight/loaded keys.
    fn request_table_foreign_keys(
        table_name: &str,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        let table_key = {
            let mut data = intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(selected) = Self::resolve_table_column_load_key(&data, table_name) else {
                return;
            };
            if !data.mark_foreign_keys_loading(&selected) {
                return;
            }
            selected
        };

        let task = ColumnLoadTask {
            table_key,
            connection: connection.clone(),
            sender: column_sender.clone(),
            foreign_keys: true,
        };

        if let Err(task) = Self::enqueue_column_load_task(task) {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                &format!(
                    "failed to enqueue foreign-key loader task for {}",
                    task.table_key
                ),
            );
            let _ = task.sender.send(ColumnLoadUpdate {
                table: task.table_key,
                columns: Vec::new(),
                column_meta: HashMap::new(),
                foreign_keys: Vec::new(),
                is_foreign_keys: true,
                cache_columns: false,
            });
            app::awake();
        }
    }

    fn resolve_table_column_load_key(
        data: &IntellisenseData,
        table_name: &str,
    ) -> Option<String> {
        let candidates = Self::table_lookup_key_candidates(table_name);
        let normalized = candidates.first()?.trim();
        if normalized.is_empty() {
            return None;
        }

        let has_unquoted_dot = Self::has_unquoted_dot(table_name);
        if has_unquoted_dot {
            let segments = Self::relation_name_segments(table_name)?;
            if segments.len() >= 2 {
                let qualifier = segments[..segments.len() - 1].join(".");
                let member = segments.last()?;
                if data.qualifier_has_member(&qualifier, member, true)
                    || data.qualifier_has_member(&qualifier, member, false)
                {
                    return Some(normalized.to_ascii_uppercase());
                }
            }
        }

        if !has_unquoted_dot && data.is_known_relation(normalized) {
            return Some(normalized.to_ascii_uppercase());
        }

        if !normalized.contains('.') {
            if let Some(default_qualifier) = data.default_qualifier() {
                if data.qualifier_has_member(default_qualifier, normalized, true)
                    || data.qualifier_has_member(default_qualifier, normalized, false)
                {
                    return Some(
                        format!("{}.{}", default_qualifier, normalized).to_ascii_uppercase(),
                    );
                }
            }
        }

        if data.is_known_relation(normalized) {
            return Some(normalized.to_ascii_uppercase());
        }

        candidates
            .iter()
            .skip(1)
            .find(|candidate| data.is_known_relation(candidate))
            .map(|candidate| candidate.to_ascii_uppercase())
    }

    fn table_lookup_key_candidates(table_name: &str) -> Vec<String> {
        let Some(segments) = Self::relation_name_segments(table_name) else {
            return Vec::new();
        };
        let normalized = segments.join(".");
        if normalized.is_empty() {
            return Vec::new();
        }

        let mut candidates = vec![normalized.clone()];
        if Self::has_unquoted_dot(table_name) {
            if let Some(last) = segments.last() {
                if !last.eq_ignore_ascii_case(&normalized) && !last.trim().is_empty() {
                    candidates.push(last.trim().to_string());
                }
            }
        }

        candidates
    }

    fn relation_name_segments(value: &str) -> Option<Vec<String>> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;

        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
                    current.push(ch);
                    if active_quote == Some(ch) {
                        if chars.peek().copied() == Some(ch) {
                            current.push(ch);
                            chars.next();
                        } else {
                            active_quote = None;
                        }
                    } else if active_quote.is_none() {
                        active_quote = Some(ch);
                    }
                }
                '.' if active_quote.is_none() => {
                    let segment = Self::strip_identifier_quotes(current.trim());
                    if !segment.is_empty() {
                        parts.push(segment);
                    } else {
                        return None;
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        if active_quote.is_some() {
            return None;
        }

        let segment = Self::strip_identifier_quotes(current.trim());
        if !segment.is_empty() {
            parts.push(segment);
        } else {
            return None;
        }

        Some(parts)
    }

    fn has_unquoted_dot(value: &str) -> bool {
        let mut chars = value.trim().chars().peekable();
        let mut active_quote: Option<char> = None;
        while let Some(ch) = chars.next() {
            match ch {
                '"' | '`' => {
                    if active_quote == Some(ch) {
                        if chars.peek().copied() == Some(ch) {
                            chars.next();
                        } else {
                            active_quote = None;
                        }
                    } else if active_quote.is_none() {
                        active_quote = Some(ch);
                    }
                }
                '.' if active_quote.is_none() => return true,
                _ => {}
            }
        }
        false
    }

    fn render_select_list_wildcard_scope(scope_name: &str) -> String {
        let segments = Self::relation_name_segments(scope_name).unwrap_or_else(|| {
            let stripped = Self::strip_identifier_quotes(scope_name);
            if stripped.trim().is_empty() {
                Vec::new()
            } else {
                vec![stripped]
            }
        });
        if segments.is_empty() {
            return String::new();
        }

        segments
            .into_iter()
            .map(|segment| Self::quote_identifier_segment_for_completion(&segment))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn quote_identifier_segment_for_completion(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "\"\"".to_string();
        }
        if sql_text::is_quoted_identifier(trimmed) {
            return trimmed.to_string();
        }
        if Self::is_unquoted_completion_identifier(trimmed) {
            return trimmed.to_string();
        }

        format!("\"{}\"", trimmed.replace('"', "\"\""))
    }

    fn completion_identifier_lookup_upper(text: &str) -> String {
        let trimmed = text.trim();
        if sql_text::is_quoted_identifier(trimmed) {
            return sql_text::strip_identifier_quotes(trimmed).to_ascii_uppercase();
        }

        match trimmed.chars().next() {
            Some('"') | Some('`') => trimmed[1..].to_ascii_uppercase(),
            _ => trimmed.to_ascii_uppercase(),
        }
    }

    fn completion_wildcard_candidate_lookup_upper(text: &str) -> String {
        let trimmed = text.trim();
        let lookup_text = trimmed.strip_suffix(".*").unwrap_or(trimmed);
        Self::completion_identifier_lookup_upper(lookup_text)
    }

    fn completion_identifiers_match(left: &str, right: &str) -> bool {
        Self::completion_identifier_lookup_upper(left) == Self::completion_identifier_lookup_upper(right)
    }

    fn is_unquoted_completion_identifier(text: &str) -> bool {
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '#')) {
            return false;
        }

        chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    }
}
