/// Outcome of loading a table's columns: aligned column names and per-column
/// display metadata, plus whether the result should be cached. Foreign keys
/// are loaded separately (lazily) so the common completion path issues a
/// single query per table.
struct LoadedColumns {
    names: Vec<String>,
    meta: Vec<ColumnMeta>,
    cache: bool,
}

impl LoadedColumns {
    fn failed() -> Self {
        Self {
            names: Vec::new(),
            meta: Vec::new(),
            cache: false,
        }
    }

    /// Build from the shared `TableColumnDetail` rows returned by every
    /// backend's structure query, preserving column order.
    fn from_details(details: Vec<crate::db::TableColumnDetail>) -> Self {
        let mut names = Vec::with_capacity(details.len());
        let mut meta = Vec::with_capacity(details.len());
        for detail in details {
            meta.push(ColumnMeta {
                type_display: detail.get_type_display(),
                nullable: detail.nullable,
                is_primary_key: detail.is_primary_key,
            });
            names.push(detail.name);
        }
        Self {
            names,
            meta,
            cache: true,
        }
    }
}

/// Convert DB-layer foreign keys into the UI representation.
fn foreign_keys_to_meta(fks: Vec<crate::db::ForeignKeyInfo>) -> Vec<ForeignKeyMeta> {
    fks.into_iter()
        .map(|fk| ForeignKeyMeta {
            columns: fk.columns,
            ref_table: fk.ref_table,
            ref_columns: fk.ref_columns,
        })
        .collect()
}

trait ColumnLoadBackend: Sync {
    fn load_columns(
        &self,
        expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        schema_and_table: Option<(&str, &str)>,
    ) -> LoadedColumns;

    /// Fetch the table's foreign keys. `Ok` (possibly empty) marks the result
    /// cacheable; `Err` means the fetch failed and should not be cached.
    fn load_foreign_keys(
        &self,
        expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        schema_and_table: Option<(&str, &str)>,
    ) -> Result<Vec<ForeignKeyMeta>, ()>;
}

struct OracleColumnLoadBackend;
struct MysqlColumnLoadBackend;

static ORACLE_COLUMN_LOAD_BACKEND: OracleColumnLoadBackend = OracleColumnLoadBackend;
static MYSQL_COLUMN_LOAD_BACKEND: MysqlColumnLoadBackend = MysqlColumnLoadBackend;

fn column_load_backend_for(db_type: crate::db::DatabaseType) -> &'static dyn ColumnLoadBackend {
    match db_type {
        crate::db::DatabaseType::Oracle => &ORACLE_COLUMN_LOAD_BACKEND,
        crate::db::DatabaseType::MySQL => &MYSQL_COLUMN_LOAD_BACKEND,
        crate::db::DatabaseType::MariaDB => &MYSQL_COLUMN_LOAD_BACKEND,
    }
}

impl ColumnLoadBackend for OracleColumnLoadBackend {
    fn load_columns(
        &self,
        _expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        _schema_and_table: Option<(&str, &str)>,
    ) -> LoadedColumns {
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                match crate::db::ObjectBrowser::get_table_structure(conn, table_key) {
                    Ok(details) => LoadedColumns::from_details(details),
                    Err(_) => LoadedColumns::failed(),
                }
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                match crate::db::ObjectBrowser::get_thin_table_structure(conn, table_key) {
                    Ok(details) => LoadedColumns::from_details(details),
                    Err(_) => LoadedColumns::failed(),
                }
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => {
                eprintln!(
                    "Warning: expected Oracle column-load session but acquired {}",
                    unexpected.db_type()
                );
                LoadedColumns::failed()
            }
        }
    }

    fn load_foreign_keys(
        &self,
        _expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        _schema_and_table: Option<(&str, &str)>,
    ) -> Result<Vec<ForeignKeyMeta>, ()> {
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                crate::db::ObjectBrowser::get_table_foreign_keys(conn, table_key)
                    .map(foreign_keys_to_meta)
                    .map_err(|_| ())
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                crate::db::ObjectBrowser::get_thin_table_foreign_keys(conn, table_key)
                    .map(foreign_keys_to_meta)
                    .map_err(|_| ())
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => {
                eprintln!(
                    "Warning: expected Oracle FK-load session but acquired {}",
                    unexpected.db_type()
                );
                Err(())
            }
        }
    }
}

impl ColumnLoadBackend for MysqlColumnLoadBackend {
    fn load_columns(
        &self,
        expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        schema_and_table: Option<(&str, &str)>,
    ) -> LoadedColumns {
        match session {
            crate::db::DbPoolSession::MySQL {
                conn: mysql_conn,
                db_type,
            } => {
                if !db_type.is_same_type_as(expected_db_type) {
                    eprintln!(
                        "Warning: expected {} column-load session but acquired {}",
                        expected_db_type.display_name(),
                        db_type.display_name()
                    );
                    return LoadedColumns::failed();
                }
                use crate::db::query::mysql_executor::MysqlObjectBrowser;
                let result = if let Some((schema, table)) = schema_and_table {
                    MysqlObjectBrowser::get_table_structure_in_schema(
                        mysql_conn.as_mut(),
                        Some(schema),
                        table,
                    )
                } else {
                    MysqlObjectBrowser::get_table_structure(mysql_conn.as_mut(), table_key)
                };
                match result {
                    Ok(details) => LoadedColumns::from_details(details),
                    Err(_) => LoadedColumns::failed(),
                }
            }
            crate::db::DbPoolSession::Oracle(_) => {
                eprintln!(
                    "Warning: expected {} column-load session but acquired {}",
                    expected_db_type.display_name(),
                    crate::db::DatabaseType::Oracle.display_name()
                );
                LoadedColumns::failed()
            }
            crate::db::DbPoolSession::OracleThin(_) => {
                eprintln!(
                    "Warning: expected {} column-load session but acquired {}",
                    expected_db_type.display_name(),
                    crate::db::DatabaseType::Oracle.display_name()
                );
                LoadedColumns::failed()
            }
        }
    }

    fn load_foreign_keys(
        &self,
        expected_db_type: crate::db::DatabaseType,
        session: &mut crate::db::DbPoolSession,
        table_key: &str,
        schema_and_table: Option<(&str, &str)>,
    ) -> Result<Vec<ForeignKeyMeta>, ()> {
        match session {
            crate::db::DbPoolSession::MySQL {
                conn: mysql_conn,
                db_type,
            } => {
                if !db_type.is_same_type_as(expected_db_type) {
                    eprintln!(
                        "Warning: expected {} FK-load session but acquired {}",
                        expected_db_type.display_name(),
                        db_type.display_name()
                    );
                    return Err(());
                }
                use crate::db::query::mysql_executor::MysqlObjectBrowser;
                let result = if let Some((schema, table)) = schema_and_table {
                    MysqlObjectBrowser::get_table_foreign_keys_in_schema(
                        mysql_conn.as_mut(),
                        Some(schema),
                        table,
                    )
                } else {
                    MysqlObjectBrowser::get_table_foreign_keys(mysql_conn.as_mut(), table_key)
                };
                result.map(foreign_keys_to_meta).map_err(|_| ())
            }
            crate::db::DbPoolSession::Oracle(_) => {
                eprintln!(
                    "Warning: expected {} FK-load session but acquired {}",
                    expected_db_type.display_name(),
                    crate::db::DatabaseType::Oracle.display_name()
                );
                Err(())
            }
            crate::db::DbPoolSession::OracleThin(_) => {
                eprintln!(
                    "Warning: expected {} FK-load session but acquired {}",
                    expected_db_type.display_name(),
                    crate::db::DatabaseType::Oracle.display_name()
                );
                Err(())
            }
        }
    }
}

impl SqlEditorWidget {
    fn is_cursor_inside_cte_explicit_column_list(
        deep_ctx: &intellisense_context::CursorContext,
        cte: &intellisense_context::CteDefinition,
    ) -> bool {
        let cursor_token_idx = deep_ctx
            .cursor_token_len
            .min(deep_ctx.statement_tokens.len());
        cte.explicit_column_range
            .is_some_and(|range| cursor_token_idx >= range.start && cursor_token_idx <= range.end)
    }

    fn collect_cte_virtual_columns_for_completion_for_db(
        deep_ctx: &intellisense_context::CursorContext,
        cte: &intellisense_context::CteDefinition,
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
        db_type: Option<crate::db::DatabaseType>,
    ) -> (Vec<String>, Vec<String>) {
        let body_tokens = intellisense_context::token_range_slice(
            deep_ctx.statement_tokens.as_ref(),
            cte.body_range,
        );
        let recursive_generated_columns =
            intellisense_context::extract_recursive_cte_generated_columns(
                deep_ctx.statement_tokens.as_ref(),
                cte.body_range.end,
            );
        let cursor_in_explicit_list =
            Self::is_cursor_inside_cte_explicit_column_list(deep_ctx, cte);
        let prefer_body_projection = cursor_in_explicit_list && !cte.body_range.is_empty();
        let should_infer_from_body = !cte.body_range.is_empty()
            && (cte.explicit_columns.is_empty() || prefer_body_projection);

        if should_infer_from_body {
            let body_tables_in_scope =
                intellisense_context::collect_tables_in_statement(body_tokens);
            let (mut columns, wildcard_tables) = Self::collect_virtual_query_projection_columns(
                body_tokens,
                &body_tables_in_scope,
                &[],
                virtual_table_columns,
                intellisense_data,
                column_sender,
                connection,
                db_type,
            );
            for column in Self::recursive_cte_anchor_columns_from_body_tokens(body_tokens) {
                Self::push_unique_completion_name(&mut columns, &column);
            }
            columns.extend(recursive_generated_columns);
            Self::dedup_column_names_case_insensitive(&mut columns);
            return (columns, wildcard_tables);
        }

        if !cte.explicit_columns.is_empty() {
            let mut columns = cte.explicit_columns.clone();
            columns.extend(recursive_generated_columns);
            Self::dedup_column_names_case_insensitive(&mut columns);
            return (columns, Vec::new());
        }

        (recursive_generated_columns, Vec::new())
    }

    #[cfg(test)]
    fn collect_cte_virtual_columns_for_completion(
        deep_ctx: &intellisense_context::CursorContext,
        cte: &intellisense_context::CteDefinition,
        virtual_table_columns: &HashMap<String, Vec<String>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) -> (Vec<String>, Vec<String>) {
        Self::collect_cte_virtual_columns_for_completion_for_db(
            deep_ctx,
            cte,
            virtual_table_columns,
            intellisense_data,
            column_sender,
            connection,
            None,
        )
    }

    fn classify_intellisense_context(
        deep_ctx: &intellisense_context::CursorContext,
        _tokens: &[SqlToken],
    ) -> SqlContext {
        sql_context_for_phase(deep_ctx.phase)
    }

    fn column_load_worker_pool() -> &'static ColumnLoadWorkerPool {
        COLUMN_LOAD_WORKER_POOL.get_or_init(Self::build_column_load_worker_pool)
    }

    fn build_column_load_worker_pool() -> ColumnLoadWorkerPool {
        let mut worker_senders = Vec::new();
        let mut worker_handles = Vec::new();
        let shutdown = Arc::new(AtomicBool::new(false));

        for idx in 0..COLUMN_LOAD_WORKER_COUNT {
            let (sender, receiver) = mpsc::channel::<ColumnLoadWorkerMessage>();
            let shutdown_for_worker = shutdown.clone();
            let spawn_result = thread::Builder::new()
                .name(format!("intellisense-column-worker-{idx}"))
                .spawn(move || {
                    while let Ok(message) = receiver.recv() {
                        match message {
                            ColumnLoadWorkerMessage::Task(task) => {
                                if shutdown_for_worker.load(Ordering::Acquire) {
                                    Self::send_empty_column_load_update(
                                        &task.sender,
                                        &task.table_key,
                                        task.foreign_keys,
                                    );
                                    continue;
                                }
                                let task_sender = task.sender.clone();
                                let task_table_key = task.table_key.clone();
                                let task_is_foreign_keys = task.foreign_keys;
                                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                    Self::process_column_load_task(task);
                                }));
                                if let Err(payload) = result {
                                    let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                                    crate::utils::logging::log_error(
                                        "sql_editor::intellisense::column_loader",
                                        &format!(
                                            "column worker panicked processing {}: {}",
                                            task_table_key, panic_msg
                                        ),
                                    );
                                    // Send empty result to unblock the loading flag.
                                    let _ = task_sender.send(ColumnLoadUpdate {
                                        table: task_table_key,
                                        columns: Vec::new(),
                                        column_meta: HashMap::new(),
                                        foreign_keys: Vec::new(),
                                        is_foreign_keys: task_is_foreign_keys,
                                        cache_columns: false,
                                    });
                                    app::awake();
                                }
                            }
                            ColumnLoadWorkerMessage::Shutdown => break,
                        }
                    }
                });

            match spawn_result {
                Ok(handle) => {
                    worker_senders.push(sender);
                    worker_handles.push(handle);
                }
                Err(err) => {
                    crate::utils::logging::log_error(
                        "sql_editor::intellisense::column_loader",
                        &format!("failed to spawn column worker {idx}: {err}"),
                    );
                }
            }
        }

        ColumnLoadWorkerPool {
            worker_senders,
            worker_handles: Mutex::new(worker_handles),
            next_worker: AtomicUsize::new(0),
            shutdown,
        }
    }

    fn enqueue_column_load_task(task: ColumnLoadTask) -> Result<(), ColumnLoadTask> {
        Self::column_load_worker_pool().enqueue(task)
    }

    pub(crate) fn shutdown_column_load_workers() {
        if let Some(pool) = COLUMN_LOAD_WORKER_POOL.get() {
            pool.shutdown();
        }
    }

    fn pool_session_context_for_column_load(
        connection: &SharedConnection,
        activity: &str,
    ) -> Result<crate::db::DbPoolSessionContext, String> {
        let mut result = crate::db::pool_session_context_for_shared_connection(
            connection,
            Some(activity),
        );
        for delay in COLUMN_LOAD_CONTEXT_RETRY_DELAYS {
            if result.is_ok() {
                break;
            }
            // This is a dedicated metadata worker and owns no mutex here. A
            // bounded sleep handles short connection hand-offs without UI
            // blocking, recursive retries, or an unbounded retry loop.
            thread::sleep(delay);
            result = crate::db::pool_session_context_for_shared_connection(
                connection,
                Some(activity),
            );
        }
        result
    }

    fn process_column_load_task(task: ColumnLoadTask) {
        let ColumnLoadTask {
            table_key,
            connection,
            scope,
            sender,
            foreign_keys,
        } = task;

        // session.md §26 / §27: IntelliSense metadata loading runs on a
        // dedicated pooled session so it cannot block the user query connection
        // mutex or disturb the tab-owned session state. Mirrors the
        // ObjectBrowser::with_pooled_object_session pattern, including the
        // pre/post-acquire scope checks that protect against a disconnect that
        // landed during the task.
        let activity = if foreign_keys {
            format!("Loading foreign keys for {}", table_key)
        } else {
            format!("Loading columns for {}", table_key)
        };

        let context = match Self::pool_session_context_for_column_load(&connection, &activity) {
            Ok(context) => context,
            Err(_) => {
                Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);
                return;
            }
        };
        let activity_guard = context.track_activity(activity);

        if !crate::db::cached_pool_session_context_matches_shared_connection(&connection, &context)
        {
            Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);
            return;
        }

        // The session and the cancel reach published over it travel as ONE
        // value (`AcquiredPoolSession`), so this column load stays reachable by
        // the cancel button for exactly as long as it holds the session.
        // The requesting TAB's scope, never the connection's: an unqualified
        // table name must resolve where that tab's statements would. The two
        // sibling metadata lookups (signature hints, bind-prompt routine
        // arguments) already acquire this way.
        let mut pool_session = match context
            .acquire_session_for_scope(
            scope.as_deref(),
            crate::db::PooledSessionPurpose::AppRead,
            &activity_guard,
        )
        {
            Ok(session) => session,
            Err(_) => {
                Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);
                return;
            }
        };

        if !crate::db::cached_pool_session_context_matches_shared_connection(&connection, &context)
        {
            Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);
            return;
        }

        let Some(session) = pool_session.session_mut() else {
            Self::send_empty_column_load_update(&sender, &table_key, foreign_keys);
            return;
        };

        let schema_and_table = Self::column_load_schema_and_table(&table_key);
        let schema_and_table_ref = schema_and_table
            .as_ref()
            .map(|(schema, table)| (schema.as_str(), table.as_str()));
        let backend = column_load_backend_for(context.connection_info.db_type);

        let update = if foreign_keys {
            match backend.load_foreign_keys(
                context.connection_info.db_type,
                session,
                table_key.as_str(),
                schema_and_table_ref,
            ) {
                Ok(fks) => ColumnLoadUpdate {
                    table: table_key,
                    columns: Vec::new(),
                    column_meta: HashMap::new(),
                    foreign_keys: fks,
                    is_foreign_keys: true,
                    cache_columns: true,
                },
                Err(()) => {
                    Self::send_empty_column_load_update(&sender, &table_key, true);
                    return;
                }
            }
        } else {
            let LoadedColumns {
                names: columns,
                meta,
                cache: cache_columns,
            } = backend.load_columns(
                context.connection_info.db_type,
                session,
                table_key.as_str(),
                schema_and_table_ref,
            );
            let column_meta: HashMap<String, ColumnMeta> = columns
                .iter()
                .zip(meta)
                .map(|(name, meta)| (name.to_uppercase(), meta))
                .collect();
            ColumnLoadUpdate {
                table: table_key,
                columns,
                column_meta,
                foreign_keys: Vec::new(),
                is_foreign_keys: false,
                cache_columns,
            }
        };

        let _ = sender.send(update);
        app::awake();
    }

    fn send_empty_column_load_update(
        sender: &mpsc::Sender<ColumnLoadUpdate>,
        table_key: &str,
        is_foreign_keys: bool,
    ) {
        let _ = sender.send(ColumnLoadUpdate {
            table: table_key.to_string(),
            columns: Vec::new(),
            column_meta: HashMap::new(),
            foreign_keys: Vec::new(),
            is_foreign_keys,
            cache_columns: false,
        });
        app::awake();
    }

    fn invoke_void_callback(callback_slot: &Arc<Mutex<Option<Box<dyn FnMut()>>>>) -> bool {
        Self::invoke_callback(callback_slot, "find/replace callback", |cb| cb())
    }

    fn invoke_file_drop_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(PathBuf)>>>>,
        path: PathBuf,
    ) -> bool {
        Self::invoke_callback(callback_slot, "file drop callback", move |cb| cb(path))
    }

    fn invoke_menu_action_callback(
        callback_slot: &Arc<Mutex<Option<Box<dyn FnMut(&'static str)>>>>,
        action: &'static str,
    ) -> bool {
        Self::invoke_callback(callback_slot, "menu action callback", move |cb| cb(action))
    }

    fn invoke_object_context_callback(
        callback_slot: &ObjectContextCallback,
        selected_text: String,
        data: IntellisenseData,
    ) -> bool {
        let callback = {
            let mut slot = Self::lock_callback_slot(callback_slot);
            slot.take()
        };

        let Some(mut cb) = callback else {
            return false;
        };

        let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(selected_text, data)));
        let mut slot = Self::lock_callback_slot(callback_slot);
        if slot.is_none() {
            *slot = Some(cb);
        }

        match call_result {
            Ok(handled) => handled,
            Err(payload) => {
                Self::log_callback_panic("object context callback", payload.as_ref());
                false
            }
        }
    }

    pub(crate) fn right_click_object_context_candidates(
        clicked_reference: Option<&str>,
        selected_text: &str,
    ) -> Vec<String> {
        let mut candidates = Vec::new();
        if let Some(reference) = clicked_reference.filter(|reference| !reference.trim().is_empty())
        {
            candidates.push(reference.to_string());
        }
        if !selected_text.trim().is_empty()
            && !candidates
                .iter()
                .any(|candidate| candidate == selected_text)
        {
            candidates.push(selected_text.to_string());
        }
        candidates
    }

    fn invoke_callback<TCallback, TInvoker>(
        callback_slot: &Arc<Mutex<Option<TCallback>>>,
        callback_name: &str,
        invoker: TInvoker,
    ) -> bool
    where
        TInvoker: FnOnce(&mut TCallback),
    {
        let callback = {
            let mut slot = Self::lock_callback_slot(callback_slot);
            slot.take()
        };

        if let Some(mut cb) = callback {
            let result = panic::catch_unwind(AssertUnwindSafe(|| invoker(&mut cb)));
            let mut slot = Self::lock_callback_slot(callback_slot);
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = result {
                Self::log_callback_panic(callback_name, payload.as_ref());
            }
            true
        } else {
            false
        }
    }

    fn lock_callback_slot<TCallback>(
        callback_slot: &Arc<Mutex<Option<TCallback>>>,
    ) -> std::sync::MutexGuard<'_, Option<TCallback>> {
        match callback_slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                eprintln!("Warning: callback slot lock was poisoned; recovering.");
                poisoned.into_inner()
            }
        }
    }

    fn should_consume_popup_confirm_key(key: Key, has_selected: bool) -> bool {
        has_selected && matches!(key, Key::Tab | Key::Enter | Key::KPEnter)
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn should_handle_enter_during_ime_composition(compose_state: i32) -> bool {
        compose_state > 0
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn selection_is_current_ime_marked_range(
        selection: Option<(i32, i32)>,
        caret: i32,
        compose_state: i32,
    ) -> bool {
        const MAX_HANGUL_MARKED_BYTES: i32 = 6;
        let Some((start, end)) = selection else {
            return false;
        };
        if start == end
            || compose_state <= 0
            || compose_state > MAX_HANGUL_MARKED_BYTES
            || caret < 0
        {
            return false;
        }
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        end == caret && start == caret.saturating_sub(compose_state)
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn selection_is_user_replacement_range(
        selection: Option<(i32, i32)>,
        caret: i32,
        compose_state: i32,
    ) -> bool {
        let Some((start, end)) = selection else {
            return false;
        };
        start != end && !Self::selection_is_current_ime_marked_range(selection, caret, compose_state)
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn key_may_change_cursor_or_selection(
        key: Key,
        shortcut_key: Key,
        ctrl_or_cmd: bool,
    ) -> bool {
        matches!(
            key,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp
                | Key::PageDown
        ) || (ctrl_or_cmd && Self::matches_alpha_shortcut(shortcut_key, 'a'))
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn ime_user_selection_replacement_text(
        event_text: &str,
        replaced_marked_text: &str,
    ) -> String {
        if !replaced_marked_text.is_empty()
            && event_text.len() > replaced_marked_text.len()
            && event_text.starts_with(replaced_marked_text)
        {
            event_text[replaced_marked_text.len()..].to_string()
        } else {
            event_text.to_string()
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn reset_ime_composition_state() {
        #[cfg(target_os = "macos")]
        {
            fltk::draw::reset_spot();
        }
        #[cfg(not(target_os = "macos"))]
        {
            app::compose_reset();
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn ime_enter_committed_text(event_text: &str, marked_text: &str) -> String {
        let committed = event_text
            .chars()
            .filter(|ch| !matches!(*ch, '\n' | '\r'))
            .collect::<String>();
        if committed.is_empty() {
            marked_text.to_string()
        } else {
            committed
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn handle_ime_enter_auto_indent(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
    ) -> bool {
        let insert_pos = editor.insert_position().max(0) as usize;
        let compose_len = app::compose_state().max(0) as usize;
        let marked_start = insert_pos.saturating_sub(compose_len);
        let marked_text = buffer
            .text_range(marked_start as i32, insert_pos as i32)
            .unwrap_or_default();
        let delete_len = app::compose()
            .unwrap_or(compose_len as i32)
            .max(0) as usize;
        let effective_delete_len = if delete_len == 0 && compose_len > 0 && !marked_text.is_empty()
        {
            compose_len
        } else {
            delete_len
        };
        let replace_start = insert_pos.saturating_sub(effective_delete_len);
        let committed = Self::ime_enter_committed_text(&app::event_text(), &marked_text);
        if replace_start != insert_pos {
            let current = buffer
                .text_range(replace_start as i32, insert_pos as i32)
                .unwrap_or_default();
            if current == committed {
                buffer.unselect();
            } else {
                buffer.replace(replace_start as i32, insert_pos as i32, &committed);
            }
        } else if !committed.is_empty() {
            buffer.unselect();
            buffer.insert(insert_pos as i32, &committed);
        } else {
            buffer.unselect();
        }

        Self::reset_ime_composition_state();
        buffer.unselect();
        let cursor = replace_start.saturating_add(committed.len());
        editor.set_insert_position(cursor.min(i32::MAX as usize) as i32);
        let indent = Self::enter_indent_for_anchor(buffer, text_shadow, cursor as i32);
        let inserted = format!("\n{indent}");
        buffer.insert(cursor.min(i32::MAX as usize) as i32, &inserted);
        editor.set_insert_position((cursor + inserted.len()).min(i32::MAX as usize) as i32);
        editor.show_insert_position();
        true
    }

    fn handle_enter_auto_indent(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
    ) -> bool {
        let selection = buffer.selection_position().map(|(start, end)| {
            let (start_pos, _) = Self::cursor_position(buffer, start);
            let (end_pos, _) = Self::cursor_position(buffer, end);
            if start_pos <= end_pos {
                (start_pos, end_pos)
            } else {
                (end_pos, start_pos)
            }
        });
        let (insert_pos, _) = Self::editor_cursor_position(editor, buffer);
        let anchor = selection
            .map(|(start, _)| start)
            .unwrap_or(insert_pos)
            .max(0);
        let indent = Self::enter_indent_for_anchor(buffer, text_shadow, anchor);
        let inserted = format!("\n{indent}");

        if let Some((start, end)) = selection {
            if start != end {
                buffer.replace(start, end, &inserted);
                editor.set_insert_position(start + inserted.len() as i32);
                editor.show_insert_position();
                return true;
            }
        }

        buffer.insert(insert_pos, &inserted);
        editor.set_insert_position(insert_pos + inserted.len() as i32);
        editor.show_insert_position();
        true
    }

    fn enter_indent_for_anchor(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        anchor: i32,
    ) -> String {
        let line_start = text_buffer_access::line_start(buffer, Some(text_shadow), anchor).max(0);
        let line_text =
            text_buffer_access::text_range(buffer, Some(text_shadow), line_start, anchor);
        Self::leading_indent_prefix(&line_text).to_string()
    }

    fn leading_indent_prefix(line_text: &str) -> &str {
        let indent_len = line_text
            .as_bytes()
            .iter()
            .take_while(|byte| matches!(**byte, b' ' | b'\t'))
            .count();
        line_text.get(..indent_len).unwrap_or("")
    }

    fn completion_insert_text(selected: &str) -> String {
        Self::condition_comparison_completion_suffix(selected)
            .unwrap_or_else(|| selected.to_string())
    }

    /// Byte offset, relative to the start of just-inserted completion text,
    /// where the caret should land. Functions are rendered with a trailing
    /// `()`; placing the caret between the parentheses lets the user type
    /// arguments immediately (matching DataGrip/Toad). All other completions
    /// place the caret at the end of the inserted text.
    fn completion_caret_offset(inserted: &str) -> usize {
        if inserted.ends_with("()") {
            inserted.len() - 1
        } else {
            inserted.len()
        }
    }

    fn completion_changes_text(
        buffer: &TextBuffer,
        start: usize,
        end: usize,
        inserted: &str,
    ) -> bool {
        if start == end {
            return !inserted.is_empty();
        }

        let start_i32 = start.min(i32::MAX as usize) as i32;
        let end_i32 = end.min(i32::MAX as usize) as i32;
        buffer
            .text_range(start_i32, end_i32)
            .map(|deleted| deleted != inserted)
            .unwrap_or(true)
    }

    fn completion_replacement_range(
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        cursor_pos: i32,
        range: Option<IntellisenseCompletionRange>,
    ) -> (usize, usize) {
        let cursor_pos_usize = cursor_pos.max(0) as usize;
        let (word, word_start, word_end) = Self::word_at_cursor(buffer, text_shadow, cursor_pos);
        Self::completion_replacement_range_from_word_bounds(
            &word,
            word_start,
            word_end,
            cursor_pos_usize,
            range.map(|value| (value.start(), value.end())),
        )
    }

    fn completion_replacement_range_from_word_bounds(
        word: &str,
        word_start: usize,
        word_end: usize,
        cursor_pos: usize,
        range: Option<(usize, usize)>,
    ) -> (usize, usize) {
        if let Some((start, end)) = range {
            if start != end {
                // A stored completion range can drift out of sync with the live
                // buffer (async popup-show timing, fast-path prefix filtering, or
                // an empty-prefix keyword popup that the user then types into). If
                // the range starts *inside* the identifier word under the cursor,
                // returning it verbatim replaces only the tail and leaves a
                // dangling prefix character — e.g. typing `pr` then choosing
                // `procedure` yields `pprocedure`. Anchor the bounds to the live
                // word so the whole typed prefix is always replaced.
                if !word.is_empty() && word_start <= cursor_pos && cursor_pos <= word_end {
                    return (start.min(word_start), end.max(cursor_pos));
                }
                return (start, end);
            }
            if word_start == cursor_pos && word_end > cursor_pos {
                return (start, word_end);
            }
            return (start, end);
        }

        if word.is_empty() {
            if word_start == cursor_pos && word_end > cursor_pos {
                return (cursor_pos, word_end);
            }
            return (cursor_pos, cursor_pos);
        }

        (word_start, cursor_pos)
    }

    fn condition_comparison_completion_suffix(selected: &str) -> Option<String> {
        let eq_idx = Self::condition_comparison_operator_index(selected)?;
        let left_expr = selected.get(..eq_idx)?;
        let dot_idx = Self::last_unquoted_dot(left_expr)?;
        selected.get(dot_idx + 1..).map(ToString::to_string)
    }

    fn condition_comparison_operator_index(text: &str) -> Option<usize> {
        let mut chars = text.char_indices().peekable();
        let mut active_quote = None;

        while let Some((idx, ch)) = chars.next() {
            if let Some(delimiter) = active_quote {
                if ch == delimiter {
                    if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => active_quote = Some(ch),
                '[' => active_quote = Some(']'),
                '=' if Self::is_spaced_condition_operator(text, idx) => return Some(idx),
                _ => {}
            }
        }

        None
    }

    fn is_spaced_condition_operator(text: &str, idx: usize) -> bool {
        text.get(..idx)
            .and_then(|prefix| prefix.chars().next_back())
            .is_some_and(char::is_whitespace)
            && text
                .get(idx + 1..)
                .and_then(|suffix| suffix.chars().next())
                .is_some_and(char::is_whitespace)
    }

    fn last_unquoted_dot(text: &str) -> Option<usize> {
        let mut last_dot = None;
        let mut chars = text.char_indices().peekable();
        let mut active_quote = None;

        while let Some((idx, ch)) = chars.next() {
            if let Some(delimiter) = active_quote {
                if ch == delimiter {
                    if chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => active_quote = Some(ch),
                '[' => active_quote = Some(']'),
                '.' => last_dot = Some(idx),
                _ => {}
            }
        }

        active_quote.is_none().then_some(last_dot).flatten()
    }

    fn column_load_schema_and_table(table_key: &str) -> Option<(String, String)> {
        let dot_idx = Self::last_unquoted_dot(table_key)?;
        let schema = table_key.get(..dot_idx)?.trim();
        let table = table_key.get(dot_idx + 1..)?.trim();
        if schema.is_empty() || table.is_empty() {
            return None;
        }

        Some((
            Self::strip_identifier_quotes(schema),
            Self::strip_identifier_quotes(table),
        ))
    }

    #[cfg(test)]
    pub(super) fn take_keyup_debounce_timeout_handle(
        keyup_debounce_handle: &Arc<Mutex<Option<crate::ui::ui_timeout::TimeoutHandle>>>,
    ) -> Option<crate::ui::ui_timeout::TimeoutHandle> {
        keyup_debounce_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(super) fn invalidate_keyup_debounce(runtime: &Arc<IntellisenseRuntimeState>) -> u64 {
        runtime.invalidate_keyup_debounce(false)
    }

    pub(super) fn invalidate_keyup_debounce_with_parse_generation(
        runtime: &Arc<IntellisenseRuntimeState>,
        invalidate_parse_generation: bool,
    ) -> u64 {
        runtime.invalidate_keyup_debounce(invalidate_parse_generation)
    }

    fn invalidate_manual_trigger_debounce_state(runtime: &Arc<IntellisenseRuntimeState>) {
        Self::invalidate_keyup_debounce_with_parse_generation(runtime, true);
    }

    fn finalize_completion_after_selection(runtime: &Arc<IntellisenseRuntimeState>) {
        runtime.clear_ui_tracking();
        Self::invalidate_keyup_debounce_with_parse_generation(runtime, true);
    }

    fn schedule_keyup_intellisense_debounce(
        runtime: &Arc<IntellisenseRuntimeState>,
        scheduled_cursor_raw: i32,
        buffer_len: i32,
        editor: &TextEditor,
        buffer: &TextBuffer,
        text_shadow: &Arc<Mutex<HighlightShadowState>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        intellisense_popup: &Arc<Mutex<IntellisensePopup>>,
        column_sender: &mpsc::Sender<ColumnLoadUpdate>,
        connection: &SharedConnection,
    ) {
        let generation = Self::invalidate_keyup_debounce_with_parse_generation(runtime, true);
        let runtime_for_timeout = runtime.clone();
        let editor_for_timeout = editor.clone();
        let buffer_for_timeout = buffer.clone();
        let text_shadow_for_timeout = text_shadow.clone();
        let intellisense_data_for_timeout = intellisense_data.clone();
        let intellisense_popup_for_timeout = intellisense_popup.clone();
        let column_sender_for_timeout = column_sender.clone();
        let connection_for_timeout = connection.clone();
        let handle = crate::ui::ui_timeout::schedule(
            Duration::from_millis(runtime.popup_delay_ms() as u64).as_secs_f64(),
            move || {
                runtime_for_timeout.take_keyup_timeout_handle();

                if runtime_for_timeout.current_keyup_generation() != generation {
                    return;
                }

                if editor_for_timeout.was_deleted() {
                    return;
                }

                // Hot-path check: for debounce invalidation we only care whether the
                // cursor offset changed, not UTF-8 boundary normalization.
                if !Self::is_same_raw_cursor_offset(
                    editor_for_timeout.insert_position(),
                    scheduled_cursor_raw,
                ) {
                    return;
                }

                if buffer_for_timeout.length() != buffer_len {
                    return;
                }

                crate::ui::sql_editor::ime_trace(|| {
                    format!(
                        "keyup-debounce fired: caret={} compose_state={} selection={:?}",
                        editor_for_timeout.insert_position(),
                        app::compose_state(),
                        buffer_for_timeout.selection_position(),
                    )
                });

                Self::trigger_intellisense(
                    &editor_for_timeout,
                    &buffer_for_timeout,
                    &text_shadow_for_timeout,
                    &intellisense_data_for_timeout,
                    &intellisense_popup_for_timeout,
                    &column_sender_for_timeout,
                    &connection_for_timeout,
                    &runtime_for_timeout,
                );
            },
        );
        runtime.set_keyup_timeout_handle(Some(handle));
    }

    fn is_same_raw_cursor_offset(current_raw: i32, scheduled_raw: i32) -> bool {
        current_raw == scheduled_raw
    }
}
