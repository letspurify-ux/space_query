trait QuickDescribeBackend: Sync {
    fn describe_object(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
    ) -> Result<QuickDescribeData, String>;
}

struct OracleQuickDescribeBackend;
struct MysqlQuickDescribeBackend;

static ORACLE_QUICK_DESCRIBE_BACKEND: OracleQuickDescribeBackend = OracleQuickDescribeBackend;
static MYSQL_QUICK_DESCRIBE_BACKEND: MysqlQuickDescribeBackend = MysqlQuickDescribeBackend;

fn quick_describe_backend_for(
    db_type: crate::db::DatabaseType,
) -> &'static dyn QuickDescribeBackend {
    match db_type {
        crate::db::DatabaseType::Oracle => &ORACLE_QUICK_DESCRIBE_BACKEND,
        crate::db::DatabaseType::MySQL => &MYSQL_QUICK_DESCRIBE_BACKEND,
        crate::db::DatabaseType::MariaDB => &MYSQL_QUICK_DESCRIBE_BACKEND,
    }
}

impl QuickDescribeBackend for OracleQuickDescribeBackend {
    fn describe_object(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        let tracked_schema = conn_guard
            .tracked_oracle_current_schema()
            .map(str::to_string);
        match conn_guard.require_live_db_connection() {
            Ok(crate::db::DbConnection::Oracle(db_conn)) => SqlEditorWidget::describe_object(
                db_conn.as_ref(),
                object_name,
                qualifier,
                tracked_schema.as_deref(),
            ),
            Ok(crate::db::DbConnection::OracleThin(db_conn)) => {
                let mut session = db_conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                crate::db::DatabaseConnection::apply_tracked_oracle_thin_current_schema(
                    &mut session,
                    tracked_schema.as_deref(),
                )?;
                SqlEditorWidget::describe_thin_object(
                    &mut session,
                    object_name,
                    qualifier,
                    tracked_schema.as_deref(),
                )
            }
            Ok(crate::db::DbConnection::MySQL { .. }) => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
            Err(message) => Err(message),
        }
    }
}

impl QuickDescribeBackend for MysqlQuickDescribeBackend {
    fn describe_object(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        conn_guard.apply_tracked_mysql_current_database()?;
        conn_guard
            .get_mysql_connection_mut()
            .ok_or_else(|| crate::db::NOT_CONNECTED_MESSAGE.to_string())
            .and_then(|mysql_conn| {
                SqlEditorWidget::describe_mysql_object(mysql_conn, object_name, qualifier)
            })
    }
}

fn non_empty(args: Vec<ProcedureArgument>) -> Option<Vec<ProcedureArgument>> {
    (!args.is_empty()).then_some(args)
}

trait SignatureBackend: Sync {
    fn resolve(
        &self,
        session: crate::db::DbPoolSession,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String>;
}

struct OracleSignatureBackend;
struct MysqlSignatureBackend;

static ORACLE_SIGNATURE_BACKEND: OracleSignatureBackend = OracleSignatureBackend;
static MYSQL_SIGNATURE_BACKEND: MysqlSignatureBackend = MysqlSignatureBackend;

fn signature_backend_for(db_type: crate::db::DatabaseType) -> &'static dyn SignatureBackend {
    match db_type {
        crate::db::DatabaseType::Oracle => &ORACLE_SIGNATURE_BACKEND,
        crate::db::DatabaseType::MySQL => &MYSQL_SIGNATURE_BACKEND,
        crate::db::DatabaseType::MariaDB => &MYSQL_SIGNATURE_BACKEND,
    }
}

fn resolve_oracle_signature_arguments(
    conn: &Connection,
    name: &str,
    qualifier: Option<&str>,
) -> Result<Option<Vec<ProcedureArgument>>, String> {
    if let Some(qualifier) = qualifier {
        if let Some(args) = ObjectBrowser::get_package_procedure_arguments(conn, qualifier, name)
            .map(non_empty)
            .map_err(|err| err.to_string())?
        {
            return Ok(Some(args));
        }
        let qualified = format!("{qualifier}.{name}");
        return ObjectBrowser::get_procedure_arguments(conn, &qualified)
            .map(non_empty)
            .map_err(|err| err.to_string());
    }
    ObjectBrowser::get_procedure_arguments(conn, name)
        .map(non_empty)
        .map_err(|err| err.to_string())
}

fn resolve_oracle_thin_signature_arguments(
    conn: &mut tns_thin::OracleThinSession,
    name: &str,
    qualifier: Option<&str>,
) -> Result<Option<Vec<ProcedureArgument>>, String> {
    if let Some(qualifier) = qualifier {
        if let Some(args) =
            ObjectBrowser::get_thin_package_procedure_arguments(conn, qualifier, name)
                .map(non_empty)?
        {
            return Ok(Some(args));
        }
        let qualified = format!("{qualifier}.{name}");
        return ObjectBrowser::get_thin_procedure_arguments(conn, &qualified).map(non_empty);
    }
    ObjectBrowser::get_thin_procedure_arguments(conn, name).map(non_empty)
}

fn resolve_mysql_signature_arguments(
    conn: &mut mysql::PooledConn,
    name: &str,
    qualifier: Option<&str>,
) -> Result<Option<Vec<ProcedureArgument>>, mysql::Error> {
    crate::db::query::mysql_executor::MysqlObjectBrowser::get_routine_arguments_in_schema(
        conn.as_mut(),
        qualifier,
        name,
    )
    .map(non_empty)
}

impl SqlEditorWidget {
    fn resolve_oracle_signature_with_timeout(
        conn: &Connection,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        let previous_timeout = conn
            .call_timeout()
            .map_err(|err| format!("Failed to read Oracle signature timeout: {err}"))?;
        conn.set_call_timeout(Some(SIGNATURE_METADATA_TIMEOUT))
            .map_err(|err| format!("Failed to apply Oracle signature timeout: {err}"))?;
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            resolve_oracle_signature_arguments(conn, name, qualifier)
        }));
        let reset_result = conn
            .set_call_timeout(previous_timeout)
            .map_err(|err| format!("Failed to restore Oracle signature timeout: {err}"));
        match result {
            Ok(Ok(args)) => reset_result.map(|()| args),
            Ok(Err(message)) => Err(match reset_result {
                Ok(()) => message,
                Err(reset_message) => format!("{message}; {reset_message}"),
            }),
            Err(payload) => {
                if let Err(message) = reset_result {
                    crate::utils::logging::log_error("signature hint", &message);
                }
                panic::resume_unwind(payload);
            }
        }
    }

    fn resolve_oracle_thin_signature_with_timeout(
        conn: &mut tns_thin::OracleThinSession,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        let previous_timeout = conn
            .call_timeout()
            .map_err(|err| format!("Failed to read Oracle Thin signature timeout: {err}"))?;
        conn.set_call_timeout(Some(SIGNATURE_METADATA_TIMEOUT))
            .map_err(|err| format!("Failed to apply Oracle Thin signature timeout: {err}"))?;
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            resolve_oracle_thin_signature_arguments(conn, name, qualifier)
        }));
        let reset_result = conn
            .set_call_timeout(previous_timeout)
            .map_err(|err| format!("Failed to restore Oracle Thin signature timeout: {err}"));
        match result {
            Ok(Ok(args)) => reset_result.map(|()| args),
            Ok(Err(message)) => Err(match reset_result {
                Ok(()) => message,
                Err(reset_message) => format!("{message}; {reset_message}"),
            }),
            Err(payload) => {
                if let Err(message) = reset_result {
                    crate::utils::logging::log_error("signature hint", &message);
                }
                panic::resume_unwind(payload);
            }
        }
    }

    fn resolve_mysql_signature_with_timeout(
        mut conn: mysql::PooledConn,
        db_type: crate::db::DatabaseType,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        let timeout_restore = match crate::db::query::mysql_executor::MysqlExecutor::apply_session_timeout_with_restore_for_db(
            &mut conn,
            Some(SIGNATURE_METADATA_TIMEOUT),
            db_type,
        ) {
            Ok(restore) => restore,
            Err(err) => {
                let restore_failed = err.restore_failed();
                let message = Self::mysql_timeout_apply_error_message(
                    &err,
                    db_type,
                    Some(SIGNATURE_METADATA_TIMEOUT),
                );
                if restore_failed {
                    crate::db::discard_mysql_pooled_connection(conn);
                }
                return Err(message);
            }
        };
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            resolve_mysql_signature_arguments(&mut conn, name, qualifier).map_err(|err| {
                Self::mysql_error_message(&err, Some(SIGNATURE_METADATA_TIMEOUT))
            })
        }));
        let reset_result = timeout_restore.map_or(Ok(()), |restore| {
            restore.restore_for_db(&mut conn, db_type).map_err(|err| {
                format!("Failed to restore {} signature timeout: {err}", db_type.display_name())
            })
        });
        match result {
            Ok(Ok(args)) => match reset_result {
                Ok(()) => Ok(args),
                Err(message) => {
                    crate::db::discard_mysql_pooled_connection(conn);
                    Err(message)
                }
            },
            Ok(Err(message)) => match reset_result {
                Ok(()) => Err(message),
                Err(reset_message) => {
                    crate::db::discard_mysql_pooled_connection(conn);
                    Err(format!("{message}; {reset_message}"))
                }
            },
            Err(payload) => {
                if let Err(message) = reset_result {
                    crate::utils::logging::log_error("signature hint", &message);
                    crate::db::discard_mysql_pooled_connection(conn);
                }
                panic::resume_unwind(payload);
            }
        }
    }
}

impl SignatureBackend for OracleSignatureBackend {
    fn resolve(
        &self,
        session: crate::db::DbPoolSession,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                SqlEditorWidget::resolve_oracle_signature_with_timeout(&conn, name, qualifier)
            }
            crate::db::DbPoolSession::OracleThin(mut conn) => {
                SqlEditorWidget::resolve_oracle_thin_signature_with_timeout(
                    &mut conn, name, qualifier,
                )
            }
            crate::db::DbPoolSession::MySQL { db_type, .. } => Err(format!(
                "Expected Oracle signature session but found {}",
                db_type.display_name()
            )),
        }
    }
}

impl SignatureBackend for MysqlSignatureBackend {
    fn resolve(
        &self,
        session: crate::db::DbPoolSession,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        match session {
            crate::db::DbPoolSession::MySQL { conn, db_type } => {
                SqlEditorWidget::resolve_mysql_signature_with_timeout(
                    conn, db_type, name, qualifier,
                )
            }
            crate::db::DbPoolSession::Oracle(_) | crate::db::DbPoolSession::OracleThin(_) => {
                Err("Expected MySQL-family signature session but found Oracle".to_string())
            }
        }
    }
}

impl SqlEditorWidget {
    /// Lookup key for a routine call: `QUALIFIER.NAME` uppercased.
    fn signature_key(call: &crate::ui::intellisense::EnclosingCall) -> String {
        crate::ui::intellisense::signature_key_for_call(call)
    }

    fn resolve_signature_label(
        db_type: crate::db::DatabaseType,
        session: crate::db::DbPoolSession,
        name: &str,
        qualifier: Option<&str>,
    ) -> Result<Option<SignatureLabel>, String> {
        let args = signature_backend_for(db_type).resolve(session, name, qualifier)?;
        Ok(args.map(|args| Self::build_signature_label(name, &args)))
    }

    pub(crate) fn signature_popup_is_visible(&self) -> bool {
        if matches!(
            self.intellisense_runtime.signature_popup_transition_state(),
            IntellisensePopupTransitionState::Showing
        ) {
            return true;
        }
        match self.signature_popup.try_lock() {
            Ok(popup) => popup.is_visible(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().is_visible(),
            Err(std::sync::TryLockError::WouldBlock) => matches!(
                self.intellisense_runtime.signature_popup_transition_state(),
                IntellisensePopupTransitionState::Showing
            ),
        }
    }

    pub(crate) fn hide_signature_popup(&self) {
        let generation = self
            .intellisense_runtime
            .next_signature_popup_request_generation();
        self.intellisense_runtime
            .set_signature_popup_transition_state(IntellisensePopupTransitionState::Idle);
        match Self::catch_signature_popup_action(|| {
            Self::try_hide_signature_popup_now(&self.signature_popup)
        }) {
            Some(false) => {}
            Some(true) | None => return,
        }
        Self::schedule_deferred_signature_popup_hide(
            self.signature_popup.clone(),
            self.intellisense_runtime.clone(),
            generation,
        );
    }

    fn try_hide_signature_popup_now(signature_popup: &Arc<Mutex<SignaturePopup>>) -> bool {
        match signature_popup.try_lock() {
            Ok(mut popup) => popup.hide(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned.into_inner().hide();
            }
            Err(std::sync::TryLockError::WouldBlock) => return false,
        }
        true
    }

    fn schedule_deferred_signature_popup_hide(
        signature_popup: Arc<Mutex<SignaturePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        generation: u64,
    ) {
        crate::ui::ui_timeout::schedule(SIGNATURE_POPUP_LOCK_RETRY_SECONDS, move || {
            if !runtime.is_current_signature_popup_request(generation) {
                return;
            }
            match Self::catch_signature_popup_action(|| {
                Self::try_hide_signature_popup_now(&signature_popup)
            }) {
                Some(false) => {}
                Some(true) | None => return,
            }
            Self::schedule_deferred_signature_popup_hide(
                signature_popup.clone(),
                runtime.clone(),
                generation,
            );
        });
    }

    fn try_show_signature_popup_now(
        signature_popup: &Arc<Mutex<SignaturePopup>>,
        editor: &TextEditor,
        label: &SignatureLabel,
        active_arg: usize,
        anchor_pos: i32,
    ) -> bool {
        match signature_popup.try_lock() {
            Ok(mut popup) => popup.show(editor, label, active_arg, anchor_pos),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned
                    .into_inner()
                    .show(editor, label, active_arg, anchor_pos);
            }
            Err(std::sync::TryLockError::WouldBlock) => return false,
        }
        true
    }

    fn catch_signature_popup_action(action: impl FnOnce() -> bool) -> Option<bool> {
        match panic::catch_unwind(AssertUnwindSafe(action)) {
            Ok(result) => Some(result),
            Err(payload) => {
                let message = Self::panic_payload_to_string(payload.as_ref());
                crate::utils::logging::log_error(
                    "signature hint",
                    &format!("signature popup action panicked: {message}"),
                );
                None
            }
        }
    }

    fn schedule_deferred_signature_popup_show(
        signature_popup: Arc<Mutex<SignaturePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        editor: TextEditor,
        label: SignatureLabel,
        active_arg: usize,
        anchor_pos: i32,
        generation: u64,
    ) {
        crate::ui::ui_timeout::schedule(SIGNATURE_POPUP_LOCK_RETRY_SECONDS, move || {
            if editor.was_deleted() || !runtime.is_current_signature_popup_request(generation) {
                return;
            }
            match Self::catch_signature_popup_action(|| {
                Self::try_show_signature_popup_now(
                    &signature_popup,
                    &editor,
                    &label,
                    active_arg,
                    anchor_pos,
                )
            }) {
                Some(false) => {}
                Some(true) | None => {
                    runtime.set_signature_popup_transition_state(
                        IntellisensePopupTransitionState::Idle,
                    );
                    return;
                }
            }
            Self::schedule_deferred_signature_popup_show(
                signature_popup.clone(),
                runtime.clone(),
                editor.clone(),
                label.clone(),
                active_arg,
                anchor_pos,
                generation,
            );
        });
    }

    fn show_signature_popup(&self, label: &SignatureLabel, active_arg: usize, anchor_pos: i32) {
        let generation = self
            .intellisense_runtime
            .next_signature_popup_request_generation();
        self.intellisense_runtime
            .set_signature_popup_transition_state(IntellisensePopupTransitionState::Showing);
        match Self::catch_signature_popup_action(|| {
            Self::try_show_signature_popup_now(
                &self.signature_popup,
                &self.editor,
                label,
                active_arg,
                anchor_pos,
            )
        }) {
            Some(false) => {}
            Some(true) | None => {
                self.intellisense_runtime.set_signature_popup_transition_state(
                    IntellisensePopupTransitionState::Idle,
                );
                return;
            }
        }
        Self::schedule_deferred_signature_popup_show(
            self.signature_popup.clone(),
            self.intellisense_runtime.clone(),
            self.editor.clone(),
            label.clone(),
            active_arg,
            anchor_pos,
            generation,
        );
    }

    pub(crate) fn delete_signature_popup_for_close(&self) {
        let generation = self
            .intellisense_runtime
            .next_signature_popup_request_generation();
        self.intellisense_runtime
            .set_signature_popup_transition_state(IntellisensePopupTransitionState::Idle);
        Self::try_delete_signature_popup_for_close(
            self.signature_popup.clone(),
            self.intellisense_runtime.clone(),
            generation,
        );
    }

    fn try_delete_signature_popup_for_close(
        signature_popup: Arc<Mutex<SignaturePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        generation: u64,
    ) {
        if !runtime.is_current_signature_popup_request(generation) {
            return;
        }
        let deleted = Self::catch_signature_popup_action(|| {
            match signature_popup.try_lock() {
                Ok(mut popup) => popup.delete_for_close(),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    poisoned.into_inner().delete_for_close();
                }
                Err(std::sync::TryLockError::WouldBlock) => return false,
            }
            true
        });
        if !matches!(deleted, Some(false)) {
            return;
        }
        crate::ui::ui_timeout::schedule(SIGNATURE_POPUP_LOCK_RETRY_SECONDS, move || {
            Self::try_delete_signature_popup_for_close(
                signature_popup.clone(),
                runtime.clone(),
                generation,
            );
        });
    }

    /// Coalesce signature refreshes and release the originating UI event before
    /// doing the bounded context parse.
    pub(crate) fn schedule_signature_hint_update(&self) {
        let generation = self
            .intellisense_runtime
            .next_signature_hint_update_generation();
        let widget = self.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            if widget.editor.was_deleted()
                || !widget
                    .intellisense_runtime
                    .is_current_signature_hint_update(generation)
            {
                return;
            }
            widget.update_signature_hint();
        });
    }

    /// Recompute the routine call enclosing the cursor and show, hide, or fetch
    /// its signature accordingly. Cheap on cache hits; spawns a background
    /// fetch (deduplicated) on a miss.
    pub(crate) fn update_signature_hint(&self) {
        if self.editor.was_deleted() {
            return;
        }
        let cursor = self.editor.insert_position().max(0) as usize;

        let db_type = self
            .intellisense_runtime
            .db_type_without_blocking(&self.connection);
        let mysql_compatible = db_type.is_mysql_or_mariadb();
        // Keep signature parsing bounded on every edit/caret move. Snap a
        // partial leading line forward so lexical state can be supplied from
        // the highlighter without cloning the full query buffer.
        const SIGNATURE_SCAN_WINDOW: usize = 4000;
        let window_start = cursor.saturating_sub(SIGNATURE_SCAN_WINDOW);
        let raw = self
            .buffer
            .text_range(window_start as i32, cursor as i32)
            .unwrap_or_default();
        let (scan_offset, scan_text) = match raw.find('\n') {
            Some(newline) if window_start > 0 => (
                window_start.saturating_add(newline).saturating_add(1),
                raw[newline + 1..].to_string(),
            ),
            _ => (window_start, raw),
        };
        let initial_lex_mode = self
            .highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .parser_lex_mode_at(scan_offset, mysql_compatible);
        let Some(mut call) = crate::ui::intellisense::enclosing_call_at_cursor_with_lexical_mode(
            &scan_text,
            scan_text.len(),
            mysql_compatible,
            initial_lex_mode.clone(),
        ) else {
            self.intellisense_runtime.clear_signature_retry();
            self.hide_signature_popup();
            return;
        };
        let local_open_paren = call.open_paren;
        call.open_paren += scan_offset;

        // Built-ins are resolved from the versioned manual catalog because
        // database argument views do not expose their parameters.
        if call.qualifier.is_none() {
            if let Some(label) = crate::ui::builtin_signatures::builtin_signature_label(
                db_type,
                &call.name,
            ) {
                self.intellisense_runtime.clear_signature_retry();
                let separator_keywords = crate::ui::builtin_signatures::
                    builtin_signature_argument_separator_keywords(db_type, &call.name)
                    .unwrap_or_default();
                let active_arg = if separator_keywords.is_empty() {
                    call.arg_index
                } else {
                    crate::ui::intellisense::call_argument_index_with_separator_keywords(
                        &scan_text,
                        scan_text.len(),
                        local_open_paren,
                        mysql_compatible,
                        initial_lex_mode,
                        separator_keywords,
                    )
                };
                self.show_signature_popup(&label, active_arg, call.open_paren as i32);
                return;
            }
        }

        let key = Self::signature_key(&call);

        enum Action {
            Show(SignatureLabel, usize),
            Hide,
            Fetch,
        }
        let action = {
            let mut data = self
                .intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match data.cached_signature(&key) {
                Some(Some(label)) => Action::Show(label.clone(), call.arg_index),
                Some(None) => Action::Hide,
                None => {
                    if data.mark_signature_pending(&key) {
                        Action::Fetch
                    } else {
                        Action::Hide
                    }
                }
            }
        };

        match action {
            Action::Show(label, active_arg) => {
                self.intellisense_runtime.clear_signature_retry();
                self.show_signature_popup(&label, active_arg, call.open_paren as i32);
            }
            Action::Hide => {
                let cached = self
                    .intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .cached_signature(&key)
                    .is_some();
                if cached {
                    self.intellisense_runtime.clear_signature_retry();
                }
                self.hide_signature_popup();
            }
            Action::Fetch => {
                self.hide_signature_popup();
                self.spawn_signature_fetch(key, call.name, call.qualifier);
            }
        }
    }

    fn spawn_signature_fetch(&self, key: String, name: String, qualifier: Option<String>) {
        let connection = self.connection.clone();
        let sender = self.ui_action_sender.clone();
        let runtime = self.intellisense_runtime.clone();
        let key_fallback = key.clone();
        let accepted = runtime.submit_signature_task(Box::new(move || {
            let activity = format!("Signature hint {name}");
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let context = crate::db::pool_session_context_for_shared_connection(
                    &connection,
                    Some(&activity),
                )?;
                let _activity_guard = crate::db::track_pool_db_activity(
                    activity,
                    context.connection_info.db_type,
                );
                if !crate::db::cached_pool_session_context_matches_shared_connection(
                    &connection,
                    &context,
                ) {
                    return Err("Signature metadata connection changed before acquire".to_string());
                }
                let session = context.acquire_session_for_current_scope()?;
                if !crate::db::cached_pool_session_context_matches_shared_connection(
                    &connection,
                    &context,
                ) {
                    return Err("Signature metadata connection changed during acquire".to_string());
                }
                let label = Self::resolve_signature_label(
                    context.connection_info.db_type,
                    session,
                    &name,
                    qualifier.as_deref(),
                )?;
                if !crate::db::cached_pool_session_context_matches_shared_connection(
                    &connection,
                    &context,
                ) {
                    return Err("Signature metadata connection changed during lookup".to_string());
                }
                Ok(label)
            }));
            let (label, cache) = match result {
                Ok(Ok(label)) => (label, true),
                Ok(Err(message)) => {
                    crate::utils::logging::log_debug("signature hint", &message);
                    (None, false)
                }
                Err(payload) => {
                    let message = Self::panic_payload_to_string(payload.as_ref());
                    crate::utils::logging::log_error(
                        "signature hint",
                        &format!("signature metadata lookup panicked: {message}"),
                    );
                    (None, false)
                }
            };
            let _ = sender.send(UiActionResult::SignatureArguments {
                key,
                label,
                cache,
            });
            app::awake();
        }));

        if !accepted {
            let _ = self.ui_action_sender.send(UiActionResult::SignatureArguments {
                key: key_fallback,
                label: None,
                cache: false,
            });
            app::awake();
        }
    }

    pub(crate) fn schedule_signature_retry(&self, key: &str) {
        let ticket = self.intellisense_runtime.next_signature_retry(key);
        let widget = self.clone();
        crate::ui::ui_timeout::schedule(ticket.delay_seconds, move || {
            if widget.editor.was_deleted()
                || !widget
                    .intellisense_runtime
                    .consume_signature_retry(ticket.generation)
            {
                return;
            }
            widget.schedule_signature_hint_update();
        });
    }

    pub fn show_quick_describe_text_dialog(title: &str, content: &str) {
        use fltk::{prelude::*, text::TextDisplay, window::Window};

        let current_group = fltk::group::Group::try_current();

        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut dialog = Window::default().with_size(760, 500).with_label(title);
        crate::ui::center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);
        dialog.begin();

        let mut display = TextDisplay::default().with_pos(10, 10).with_size(740, 440);
        display.set_color(theme::editor_bg());
        display.set_text_color(theme::text_primary());
        display.set_text_font(crate::ui::configured_editor_profile().normal);
        display.set_text_size(crate::ui::configured_ui_font_size());
        theme::style_text_display_scrollbars(&display);

        let mut buffer = fltk::text::TextBuffer::default();
        buffer.set_text(content);
        display.set_buffer(buffer);

        let close_btn_x = crate::utils::arithmetic::safe_div(760 - BUTTON_WIDTH, 2);
        let mut close_btn = fltk::button::Button::default()
            .with_pos(close_btn_x, 460)
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Close");
        close_btn.set_color(theme::button_secondary());
        close_btn.set_label_color(theme::text_primary());

        let (sender, receiver) = mpsc::channel::<()>();
        close_btn.set_callback(move |_| {
            let _ = sender.send(());
            app::awake();
        });

        dialog.end();
        dialog.show();
        fltk::group::Group::set_current(current_group.as_ref());

        while dialog.shown() {
            fltk::app::wait();
            if receiver.try_recv().is_ok() {
                dialog.hide();
            }
        }

        // Explicitly destroy top-level dialog widgets to release native resources.
        Window::delete(dialog);
    }
    pub fn hide_intellisense_on_outside_click(&self, x: i32, y: i32) {
        if matches!(
            self.intellisense_runtime.popup_transition_state(),
            IntellisensePopupTransitionState::Showing
        ) {
            Self::schedule_deferred_outside_click_popup_hide(
                self.intellisense_popup.clone(),
                self.intellisense_runtime.clone(),
                x,
                y,
                INTELLISENSE_DEFERRED_HIDE_RETRIES,
            );
            return;
        }
        let mut popup = self
            .intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let popup_visible = popup.is_visible();
        if !popup_visible {
            return;
        }
        let click_inside_popup = popup_visible && popup.contains_point(x, y);
        if Self::should_ignore_external_hide_click(popup_visible, click_inside_popup) {
            return;
        }
        popup.hide();
        drop(popup);
        Self::clear_intellisense_state_for_external_hide(&self.intellisense_runtime);
    }

    pub fn hide_intellisense_popup(&self) {
        let mut popup = self
            .intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !popup.is_visible() {
            return;
        }
        popup.hide();
        drop(popup);
        Self::clear_intellisense_state_for_external_hide(&self.intellisense_runtime);
    }

    fn can_try_hide_intellisense_popup(state: IntellisensePopupTransitionState) -> bool {
        matches!(state, IntellisensePopupTransitionState::Idle)
    }

    pub fn try_hide_intellisense_popup(&self) {
        if !Self::can_try_hide_intellisense_popup(self.intellisense_runtime.popup_transition_state())
        {
            return;
        }

        let Ok(mut popup) = self.intellisense_popup.try_lock() else {
            return;
        };
        if !popup.is_visible() {
            return;
        }
        popup.hide();
        drop(popup);
        Self::clear_intellisense_state_for_external_hide(&self.intellisense_runtime);
    }

    #[allow(dead_code)]
    pub fn update_intellisense_data(&mut self, data: IntellisenseData) {
        let mut data = data;
        data.rebuild_indices();
        *self
            .intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = data;
    }

    pub fn get_intellisense_data(&self) -> Arc<Mutex<IntellisenseData>> {
        self.intellisense_data.clone()
    }
    pub fn show_intellisense(&self) {
        Self::trigger_intellisense(
            &self.editor,
            &self.buffer,
            &self.highlight_shadow,
            &self.intellisense_data,
            &self.intellisense_popup,
            &self.column_sender,
            &self.connection,
            &self.intellisense_runtime,
        );
    }

    pub fn quick_describe_at_cursor(&self) {
        let (cursor_pos, _) = Self::editor_cursor_position(&self.editor, &self.buffer);
        let Some((_word, raw_word, start, _)) =
            Self::identifier_at_position_with_raw(&self.buffer, &self.highlight_shadow, cursor_pos)
        else {
            return;
        };
        let preferred_db_type = Some(
            self.intellisense_runtime
                .db_type_without_blocking(&self.connection),
        );
        let (qualifier, raw_qualifier) = Self::qualifiers_before_word(
            &self.buffer,
            &self.highlight_shadow,
            start as usize,
            preferred_db_type,
        );
        let object_name = if let Some(ref qualifier) = raw_qualifier {
            format!("{}.{}", qualifier, raw_word)
        } else {
            raw_word.clone()
        };

        let connection = self.connection.clone();
        let sender = self.ui_action_sender.clone();
        let sender_for_thread = sender.clone();
        set_cursor(Cursor::Wait);
        app::flush();
        let object_name_for_thread = object_name.clone();
        let spawn_result = thread::Builder::new()
            .name("quick-describe".to_string())
            .spawn(move || {
                let sender_fallback = sender_for_thread.clone();
                let object_name_fallback = object_name_for_thread.clone();
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    // Try to acquire connection lock without blocking
                    let Some(mut conn_guard) = crate::db::try_lock_connection_with_activity(
                        &connection,
                        format!("Quick describe {}", object_name_for_thread),
                    ) else {
                        // Query is already running, notify user
                        let _ = sender_for_thread.send(UiActionResult::QueryAlreadyRunning);
                        app::awake();
                        return;
                    };

                    let describe_qualifier = raw_qualifier.as_deref().or(qualifier.as_deref());
                    let result = Self::describe_object_for_current_db(
                        &mut conn_guard,
                        &raw_word,
                        describe_qualifier,
                    );

                    let _ = sender_for_thread.send(UiActionResult::QuickDescribe {
                        object_name: object_name_for_thread,
                        result,
                    });
                    app::awake();
                }));
                if let Err(payload) = result {
                    let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                    crate::utils::logging::log_error(
                        "sql_editor::intellisense::quick_describe",
                        &format!("quick describe thread panicked: {}", panic_msg),
                    );
                    let _ = sender_fallback.send(UiActionResult::QuickDescribe {
                        object_name: object_name_fallback,
                        result: Err(format!("Internal error: {}", panic_msg)),
                    });
                    app::awake();
                }
            });

        if let Err(err) = spawn_result {
            let message = format!("Failed to start quick describe task: {err}");
            crate::utils::logging::log_error("sql_editor::intellisense::quick_describe", &message);
            let _ = sender.send(UiActionResult::QuickDescribe {
                object_name,
                result: Err(message),
            });
            app::awake();
        }
    }

    fn describe_object_for_current_db(
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        quick_describe_backend_for(conn_guard.db_type()).describe_object(
            conn_guard,
            object_name,
            qualifier,
        )
    }

    fn describe_mysql_object(
        conn: &mut mysql::Conn,
        object_name: &str,
        qualifier: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        use crate::db::query::mysql_executor::MysqlObjectBrowser;

        let object_name = Self::strip_identifier_quotes(object_name);
        let qualifier = qualifier.map(Self::strip_identifier_quotes);
        let qualifier = qualifier.as_deref();
        let qualified_name = qualifier
            .map(|schema| format!("{schema}.{}", object_name))
            .unwrap_or_else(|| object_name.clone());

        if let Ok(columns) =
            MysqlObjectBrowser::get_table_structure_in_schema(conn, qualifier, &object_name)
        {
            if !columns.is_empty() {
                return Ok(QuickDescribeData::TableColumns(columns));
            }
        }

        let mut object_types =
            MysqlObjectBrowser::get_object_types_in_schema(conn, qualifier, &object_name)
                .map_err(|err| err.to_string())?;
        if object_types.is_empty() {
            return Err(format!(
                "Object not found or not accessible: {}",
                qualified_name.to_uppercase()
            ));
        }

        object_types.sort_by_key(|object_type| Self::quick_describe_type_priority(object_type));

        for object_type in object_types {
            let object_type_upper = object_type.to_uppercase();
            match object_type_upper.as_str() {
                "TABLE" | "VIEW" => {
                    if let Ok(columns) = MysqlObjectBrowser::get_table_structure_in_schema(
                        conn,
                        qualifier,
                        &object_name,
                    ) {
                        if !columns.is_empty() {
                            return Ok(QuickDescribeData::TableColumns(columns));
                        }
                    }
                }
                "FUNCTION" | "PROCEDURE" => {
                    let args = MysqlObjectBrowser::get_routine_arguments_in_schema(
                        conn,
                        qualifier,
                        &object_name,
                    )
                    .map_err(|err| err.to_string())?;
                    let content =
                        Self::format_routine_details(&qualified_name, &object_type_upper, &args);
                    return Ok(QuickDescribeData::Text {
                        title: format!(
                            "Describe: {} ({})",
                            qualified_name.to_uppercase(),
                            object_type_upper
                        ),
                        content,
                    });
                }
                _ => {
                    let ddl = MysqlObjectBrowser::get_create_object_in_schema(
                        conn,
                        qualifier,
                        &object_type_upper,
                        &object_name,
                    )
                    .map_err(|err| err.to_string())?;
                    if !ddl.trim().is_empty() {
                        return Ok(QuickDescribeData::Text {
                            title: format!(
                                "Describe: {} ({})",
                                qualified_name.to_uppercase(),
                                object_type_upper
                            ),
                            content: ddl,
                        });
                    }
                }
            }
        }

        Err(format!(
            "Object not found or not accessible: {}",
            qualified_name.to_uppercase()
        ))
    }
}
