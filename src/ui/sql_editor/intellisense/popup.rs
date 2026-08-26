trait QuickDescribeBackend: Sync {
    /// `scope` is the schema/database the requesting query tab has selected,
    /// which is what an unqualified name must resolve against.
    ///
    /// `query_timeout` is the requesting tab's, and every backend must apply it
    /// to the calls it makes. This read runs on the connection's OWN session —
    /// the one every tab's work on that connection queues behind — and it had
    /// no bound at all: a describe against a stalled server held the connection
    /// mutex until someone cancelled it from the activity view. It is the tab's
    /// timeout rather than a fixed metadata one because this is a deliberate
    /// user action whose result opens a tab, exactly like F6, and the timeout
    /// box is where the user says how long their own work may take. (The
    /// signature hints, which the user never asked for, keep their own fixed
    /// `SIGNATURE_METADATA_TIMEOUT` for the opposite reason.)
    fn describe_object(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
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
        scope: Option<&str>,
        query_timeout: Option<Duration>,
    ) -> Result<QuickDescribeData, String> {
        // The lookup name carries the schema, so the tab's scope only has to
        // reach the name — the shared live session keeps its own tracked
        // schema and nothing here has to change it. The name resolves the
        // same way a session does (tab scope, else this connection's own
        // schema), so it never lands in whatever schema another tab left the
        // shared session in.
        let lookup_schema = conn_guard.oracle_session_schema_for_scope(scope);
        let tracked_schema = conn_guard
            .tracked_oracle_current_schema()
            .map(str::to_string);
        match conn_guard.require_live_db_connection() {
            Ok(crate::db::DbConnection::Oracle(db_conn)) => {
                SqlEditorWidget::run_oracle_action_with_timeout(
                    db_conn,
                    query_timeout,
                    "Quick describe",
                    |db_conn| {
                        SqlEditorWidget::describe_object(
                            db_conn.as_ref(),
                            object_name,
                            qualifier,
                            lookup_schema.as_deref(),
                        )
                    },
                )
            }
            Ok(crate::db::DbConnection::OracleThin(db_conn)) => {
                let mut session = db_conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                SqlEditorWidget::run_oracle_thin_action_with_timeout(
                    &mut session,
                    query_timeout,
                    |session| {
                        // Go to Declaration describes ONE object by name:
                        // resolving it in the login schema because the tab's is
                        // gone would describe a different object of the same
                        // name and say nothing. Inside the timeout because it is
                        // a server round trip like the describe itself.
                        crate::db::DatabaseConnection::apply_tracked_oracle_thin_current_schema(
                            session,
                            tracked_schema.as_deref(),
                        )?
                        .require_applied(crate::db::DatabaseType::Oracle)?;
                        SqlEditorWidget::describe_thin_object(
                            session,
                            object_name,
                            qualifier,
                            lookup_schema.as_deref(),
                        )
                    },
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
        scope: Option<&str>,
        query_timeout: Option<Duration>,
    ) -> Result<QuickDescribeData, String> {
        // Name the database in the lookup instead of switching the session to
        // it: MySQL 8 and MariaDB fold `DATABASE()` into a prepared
        // INFORMATION_SCHEMA statement when it is first prepared, so a cached
        // describe statement keeps answering for the database the session was
        // in back then.
        let lookup_database = conn_guard.mysql_database_for_scope(scope).to_string();
        let lookup_database = (!lookup_database.is_empty()).then_some(lookup_database);
        // The session half of the execution ceremony and nothing else: this
        // read must be bounded by the tab's timeout like any other work on the
        // connection's own session, and it must not touch that session's
        // database (see above) or publish a cancel target (the connection lock
        // already published one for whatever holds it).
        SqlEditorWidget::run_mysql_main_connection_action(
            conn_guard,
            None,
            query_timeout,
            "Quick describe",
            |conn_guard| {
                conn_guard
                    .get_mysql_connection_mut()
                    .ok_or_else(|| crate::db::NOT_CONNECTED_MESSAGE.to_string())
                    .and_then(|mysql_conn| {
                        SqlEditorWidget::describe_mysql_object(
                            mysql_conn,
                            object_name,
                            qualifier,
                            lookup_database.as_deref(),
                        )
                    })
            },
        )
    }
}

fn non_empty(args: Vec<ProcedureArgument>) -> Option<Vec<ProcedureArgument>> {
    (!args.is_empty()).then_some(args)
}

trait SignatureBackend: Sync {
    /// `usability` is the session OWNER's flag: a backend that leaves the
    /// session in an unknown state sets it, and the owner closes the session
    /// instead of returning it to the pool. It is passed alongside the borrow
    /// rather than taken from the session, because a borrower cannot hold both
    /// at once -- see `PoolSessionUsability`.
    fn resolve(
        &self,
        session: &mut crate::db::DbPoolSession,
        usability: &crate::db::PoolSessionUsability,
        name: &str,
        qualifier: Option<&str>,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
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
    routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
) -> Result<Option<Vec<ProcedureArgument>>, mysql::Error> {
    // The call site names the namespace when the statement is a CALL; a call
    // site that could not be read asks for whichever routine carries the name
    // (function preferred when both do).
    match routine_kind {
        Some(kind) => {
            crate::db::query::mysql_executor::MysqlObjectBrowser::get_routine_arguments_in_schema(
                conn.as_mut(),
                qualifier,
                name,
                kind,
            )
        }
        None => crate::db::query::mysql_executor::MysqlObjectBrowser::
            get_routine_arguments_in_schema_any_kind(conn.as_mut(), qualifier, name),
    }
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

    /// `usability` is how this borrower says the session must be CLOSED rather
    /// than returned to the pool: applying the signature timeout and failing to
    /// restore it leaves the session carrying settings nobody has accounted
    /// for. It used to take the session by value to do that, which is what made
    /// the whole signature backend own a session it only reads -- and an owned
    /// session is one whose cancel registration a caller has to remember to
    /// carry alongside it. See `PoolSessionUsability`.
    fn resolve_mysql_signature_with_timeout(
        conn: &mut mysql::PooledConn,
        usability: &crate::db::PoolSessionUsability,
        db_type: crate::db::DatabaseType,
        name: &str,
        qualifier: Option<&str>,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        let timeout_restore = match crate::db::query::mysql_executor::MysqlExecutor::apply_session_timeout_with_restore_for_db(
            conn,
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
                    usability.mark_unusable();
                }
                return Err(message);
            }
        };
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            resolve_mysql_signature_arguments(conn, name, qualifier, mysql_routine_kind).map_err(|err| {
                Self::mysql_error_message(&err, Some(SIGNATURE_METADATA_TIMEOUT))
            })
        }));
        let reset_result = timeout_restore.map_or(Ok(()), |restore| {
            restore.restore_for_db(conn, db_type).map_err(|err| {
                format!("Failed to restore {} signature timeout: {err}", db_type.display_name())
            })
        });
        match result {
            Ok(Ok(args)) => match reset_result {
                Ok(()) => Ok(args),
                Err(message) => {
                    usability.mark_unusable();
                    Err(message)
                }
            },
            Ok(Err(message)) => match reset_result {
                Ok(()) => Err(message),
                Err(reset_message) => {
                    usability.mark_unusable();
                    Err(format!("{message}; {reset_message}"))
                }
            },
            Err(payload) => {
                if let Err(message) = reset_result {
                    crate::utils::logging::log_error("signature hint", &message);
                    // Read by the session owner's own drop, so this still
                    // closes the session while the panic unwinds past here.
                    usability.mark_unusable();
                }
                panic::resume_unwind(payload);
            }
        }
    }
}

impl SignatureBackend for OracleSignatureBackend {
    fn resolve(
        &self,
        session: &mut crate::db::DbPoolSession,
        _usability: &crate::db::PoolSessionUsability,
        name: &str,
        qualifier: Option<&str>,
        _mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                SqlEditorWidget::resolve_oracle_signature_with_timeout(conn, name, qualifier)
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                SqlEditorWidget::resolve_oracle_thin_signature_with_timeout(
                    conn, name, qualifier,
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
        session: &mut crate::db::DbPoolSession,
        usability: &crate::db::PoolSessionUsability,
        name: &str,
        qualifier: Option<&str>,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        match session {
            crate::db::DbPoolSession::MySQL { conn, db_type } => {
                SqlEditorWidget::resolve_mysql_signature_with_timeout(
                    conn,
                    usability,
                    *db_type,
                    name,
                    qualifier,
                    mysql_routine_kind,
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
        session: &mut crate::db::AcquiredPoolSession,
        name: &str,
        qualifier: Option<&str>,
        display_name: &str,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) -> Result<Option<SignatureLabel>, String> {
        let args = Self::resolve_routine_arguments(
            db_type,
            session,
            name,
            qualifier,
            mysql_routine_kind,
        )?;
        Ok(args.map(|args| Self::build_signature_label(display_name, &args)))
    }

    /// The routine's parameter list itself, for the caller that needs the types
    /// rather than a label to draw — the bind parameter prompt.
    pub(crate) fn resolve_routine_arguments(
        db_type: crate::db::DatabaseType,
        session: &mut crate::db::AcquiredPoolSession,
        name: &str,
        qualifier: Option<&str>,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) -> Result<Option<Vec<ProcedureArgument>>, String> {
        // Cloned BEFORE the borrow, because the borrow is exclusive: this is
        // the whole reason the flag is a shared value rather than a method on
        // the session.
        let usability = session.usability();
        let Some(session) = session.session_mut() else {
            return Err("The signature metadata session was already given up".to_string());
        };
        signature_backend_for(db_type).resolve(session, &usability, name, qualifier, mysql_routine_kind)
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
            Some(true) => return,
            None => return,
        }
        Self::schedule_deferred_signature_popup_hide(
            self.signature_popup.clone(),
            self.intellisense_runtime.clone(),
            generation,
            SIGNATURE_POPUP_LOCK_MAX_RETRIES,
        );
    }

    /// Explicitly dismiss the hint until the next editor-originated refresh.
    /// This prevents late metadata results and retry timers from resurrecting
    /// a popup closed by focus, window, pointer, or Escape handling.
    pub(crate) fn dismiss_signature_popup(&self) {
        self.intellisense_runtime.suppress_signature_hints();
        self.hide_signature_popup();
    }

    pub(crate) fn editor_contains_root_point(&self, x: i32, y: i32) -> bool {
        let Some(window) = self.editor.window() else {
            return false;
        };
        let left = window.x_root().saturating_add(self.editor.x());
        let top = window.y_root().saturating_add(self.editor.y());
        let right = left.saturating_add(self.editor.w());
        let bottom = top.saturating_add(self.editor.h());
        x >= left && x < right && y >= top && y < bottom
    }

    /// Entry point for focus-loss style hides coming from outside the
    /// intellisense module (e.g. main-window Deactivate).
    pub(crate) fn hide_signature_popup_after_focus_settles(&self) {
        self.schedule_deferred_signature_unfocus_hide(INTELLISENSE_DEFERRED_HIDE_RETRIES);
    }

    fn should_defer_signature_unfocus_hide(
        completion_transition: IntellisensePopupTransitionState,
        signature_transition: IntellisensePopupTransitionState,
    ) -> bool {
        matches!(completion_transition, IntellisensePopupTransitionState::Showing)
            || matches!(signature_transition, IntellisensePopupTransitionState::Showing)
    }

    /// Match the completion popup's unfocus behavior: close immediately once
    /// both popup show transitions have settled. A show transition can briefly
    /// unfocus the editor on macOS, so that case still uses the deferred check.
    pub(crate) fn hide_signature_popup_on_editor_unfocus(&self) {
        if Self::should_defer_signature_unfocus_hide(
            self.intellisense_runtime.popup_transition_state(),
            self.intellisense_runtime
                .signature_popup_transition_state(),
        ) {
            self.schedule_deferred_signature_unfocus_hide(INTELLISENSE_DEFERRED_HIDE_RETRIES);
        } else {
            self.dismiss_signature_popup();
        }
    }

    /// Hide the signature popup on editor unfocus only when focus actually
    /// left the editor. Showing the completion popup window briefly pulls
    /// focus (macOS key-window flicker), which must not kill the hint; the
    /// completion popup's own unfocus hide uses the same deferred check.
    /// Rechecks are spaced in real time because the key-window round trip can
    /// take more than one event-loop turn.
    pub(crate) fn schedule_deferred_signature_unfocus_hide(&self, retries_left: u8) {
        const SIGNATURE_UNFOCUS_RECHECK_SECONDS: f64 = 0.05;
        let widget = self.clone();
        crate::ui::ui_timeout::schedule(SIGNATURE_UNFOCUS_RECHECK_SECONDS, move || {
            if widget.editor.was_deleted() {
                return;
            }
            if widget.editor.has_focus() && widget.editor.active_r() {
                return;
            }
            if retries_left > 0 {
                widget.schedule_deferred_signature_unfocus_hide(retries_left - 1);
                return;
            }
            widget.dismiss_signature_popup();
        });
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
        retries_remaining: u8,
    ) {
        let Some(next_retries_remaining) =
            Self::reserve_signature_popup_lock_retry(retries_remaining)
        else {
            Self::log_signature_popup_lock_retry_exhausted("hide");
            return;
        };
        crate::ui::ui_timeout::schedule(SIGNATURE_POPUP_LOCK_RETRY_SECONDS, move || {
            if !runtime.is_current_signature_popup_request(generation) {
                return;
            }
            match Self::catch_signature_popup_action(|| {
                Self::try_hide_signature_popup_now(&signature_popup)
            }) {
                Some(false) => {}
                Some(true) => return,
                None => return,
            }
            Self::schedule_deferred_signature_popup_hide(
                signature_popup.clone(),
                runtime.clone(),
                generation,
                next_retries_remaining,
            );
        });
    }

    fn reserve_signature_popup_lock_retry(retries_remaining: u8) -> Option<u8> {
        retries_remaining.checked_sub(1)
    }

    fn log_signature_popup_lock_retry_exhausted(operation: &str) {
        crate::utils::logging::log_warning(
            "signature hint",
            &format!("signature popup {operation} abandoned after lock retry exhaustion"),
        );
    }

    fn try_show_signature_popup_now(
        signature_popup: &Arc<Mutex<SignaturePopup>>,
        editor: &TextEditor,
        label: &SignatureLabel,
        active_arg: usize,
        anchor_pos: i32,
    ) -> bool {
        let restore_editor_focus = match signature_popup.try_lock() {
            Ok(mut popup) => popup.show(editor, label, active_arg, anchor_pos),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned
                    .into_inner()
                    .show(editor, label, active_arg, anchor_pos)
            }
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        if restore_editor_focus {
            let mut editor = editor.clone();
            let _ = editor.take_focus();
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
        retries_remaining: u8,
    ) {
        let Some(next_retries_remaining) =
            Self::reserve_signature_popup_lock_retry(retries_remaining)
        else {
            runtime
                .set_signature_popup_transition_state(IntellisensePopupTransitionState::Idle);
            Self::log_signature_popup_lock_retry_exhausted("show");
            return;
        };
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
                next_retries_remaining,
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
            SIGNATURE_POPUP_LOCK_MAX_RETRIES,
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
            SIGNATURE_POPUP_LOCK_MAX_RETRIES,
        );
    }

    fn try_delete_signature_popup_for_close(
        signature_popup: Arc<Mutex<SignaturePopup>>,
        runtime: Arc<IntellisenseRuntimeState>,
        generation: u64,
        retries_remaining: u8,
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
        let Some(next_retries_remaining) =
            Self::reserve_signature_popup_lock_retry(retries_remaining)
        else {
            Self::log_signature_popup_lock_retry_exhausted("delete");
            return;
        };
        crate::ui::ui_timeout::schedule(SIGNATURE_POPUP_LOCK_RETRY_SECONDS, move || {
            Self::try_delete_signature_popup_for_close(
                signature_popup.clone(),
                runtime.clone(),
                generation,
                next_retries_remaining,
            );
        });
    }

    /// Coalesce signature refreshes and release the originating UI event before
    /// doing the bounded context parse.
    pub(crate) fn schedule_signature_hint_update(&self) {
        self.intellisense_runtime.resume_signature_hints();
        self.schedule_signature_hint_refresh();
    }

    /// Refresh requested by metadata completion or retry. Unlike an editor
    /// event, this must honor a prior explicit dismissal.
    pub(crate) fn schedule_signature_hint_refresh(&self) {
        if self.intellisense_runtime.signature_hints_suppressed() {
            return;
        }
        let generation = self
            .intellisense_runtime
            .next_signature_hint_update_generation();
        let widget = self.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            if widget.editor.was_deleted()
                || widget.intellisense_runtime.signature_hints_suppressed()
                || !widget.editor.has_focus()
                || !widget.editor.active_r()
                || !widget.editor.visible_r()
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
        if self.editor.was_deleted() || self.intellisense_runtime.signature_hints_suppressed() {
            return;
        }
        let cursor = self.editor.insert_position().max(0) as usize;

        let db_type = self.current_db_type();
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
        // On the MySQL family the call site itself names the routine
        // namespace (`CALL name(` is a procedure, anything else a function);
        // carried into the lookup AND the cache key so a name that is both at
        // once never shows one namespace's signature at the other's call.
        let mysql_routine_kind = mysql_compatible
            .then(|| {
                crate::ui::bind_prompt::mysql_call_site_routine_kind(&scan_text, local_open_paren)
            })
            .flatten();
        call.open_paren += scan_offset;

        // Built-ins are resolved from the versioned manual catalog because
        // database argument views do not expose their parameters.
        if call.qualifier.is_none() && !call.name_quoted {
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

        let mut key = Self::signature_key(&call);
        if let Some(kind) = mysql_routine_kind {
            key.push('#');
            key.push_str(kind.as_routine_type());
        }

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
                let display_name = call.lookup_name.clone();
                let (lookup_name, lookup_qualifier) =
                    if db_type.preserves_quoted_routine_lookup_spelling() {
                        (call.lookup_name, call.lookup_qualifier)
                    } else {
                        (call.name, call.qualifier)
                    };
                self.spawn_signature_fetch(
                    key,
                    display_name,
                    lookup_name,
                    lookup_qualifier,
                    mysql_routine_kind,
                );
            }
        }
    }

    fn spawn_signature_fetch(
        &self,
        key: String,
        display_name: String,
        lookup_name: String,
        qualifier: Option<String>,
        mysql_routine_kind: Option<crate::db::query::mysql_executor::MysqlRoutineKind>,
    ) {
        let Some(connection) = self.bound_connection() else {
            let _ = self.ui_action_sender.send(UiActionResult::SignatureArguments {
                key,
                label: None,
                cache: false,
            });
            app::awake();
            return;
        };
        // Same scope the tab's statements run in: an unqualified routine's
        // signature must come from where the call would actually resolve.
        let tab_scope = self.connection_binding.snapshot().scope;
        let sender = self.ui_action_sender.clone();
        let runtime = self.intellisense_runtime.clone();
        let key_fallback = key.clone();
        let accepted = runtime.submit_signature_task(Box::new(move || {
            let activity = format!("Signature hint {display_name}");
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let context = crate::db::pool_session_context_for_shared_connection(
                    &connection,
                    Some(&activity),
                )?;
                let activity_guard = context.track_activity(activity);
                if !crate::db::cached_pool_session_context_matches_shared_connection(
                    &connection,
                    &context,
                ) {
                    return Err("Signature metadata connection changed before acquire".to_string());
                }
                // Session and cancel reach as one value -- see
                // `AcquiredPoolSession`.
                let mut acquired =
                    context.acquire_session_for_scope(
                    tab_scope.as_deref(),
                    crate::db::PooledSessionPurpose::AppRead,
                    &activity_guard,
                )?;
                if !crate::db::cached_pool_session_context_matches_shared_connection(
                    &connection,
                    &context,
                ) {
                    return Err("Signature metadata connection changed during acquire".to_string());
                }
                let label = Self::resolve_signature_label(
                    context.connection_info.db_type,
                    &mut acquired,
                    &lookup_name,
                    qualifier.as_deref(),
                    &display_name,
                    mysql_routine_kind,
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
        if self.intellisense_runtime.signature_hints_suppressed() {
            return;
        }
        let Some(ticket) = self.intellisense_runtime.next_signature_retry(key) else {
            crate::utils::logging::log_debug(
                "signature hint",
                &format!("signature metadata retry limit reached for {key}"),
            );
            return;
        };
        let widget = self.clone();
        crate::ui::ui_timeout::schedule(ticket.delay_seconds, move || {
            if widget.editor.was_deleted()
                || widget.intellisense_runtime.signature_hints_suppressed()
                || !widget
                    .intellisense_runtime
                    .consume_signature_retry(ticket.generation)
            {
                return;
            }
            widget.schedule_signature_hint_refresh();
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
        close_btn.set_color(theme::button_dark());
        close_btn.set_label_color(theme::text_primary());
        theme::install_button_hover(&mut close_btn);

        let (sender, receiver) = mpsc::channel::<()>();
        close_btn.set_callback(move |_| {
            let _ = sender.send(());
            app::awake();
        });

        dialog.end();
        dialog.show();
        fltk::group::Group::set_current(current_group.as_ref());

        crate::ui::break_active_grab_for_modal();
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
        let mut popup = match self.intellisense_popup.try_lock() {
            Ok(popup) => popup,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                Self::schedule_deferred_outside_click_popup_hide(
                    self.intellisense_popup.clone(),
                    self.intellisense_runtime.clone(),
                    x,
                    y,
                    INTELLISENSE_DEFERRED_HIDE_RETRIES,
                );
                return;
            }
        };
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
        let mut popup = match self.intellisense_popup.try_lock() {
            Ok(popup) => popup,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                Self::request_intellisense_popup_hide(
                    &self.intellisense_popup,
                    &self.intellisense_runtime,
                );
                Self::clear_intellisense_state_for_external_hide(&self.intellisense_runtime);
                return;
            }
        };
        if !popup.is_visible() {
            return;
        }
        popup.hide();
        drop(popup);
        Self::clear_intellisense_state_for_external_hide(&self.intellisense_runtime);
    }

    /// Deactivation can be a transient side effect of showing the completion
    /// window on macOS. Wait for focus to settle before treating it as a real
    /// focus loss.
    pub(crate) fn hide_intellisense_popup_after_focus_settles(&self) {
        Self::schedule_deferred_unfocus_popup_hide(
            self.editor.clone(),
            self.intellisense_popup.clone(),
            self.intellisense_runtime.clone(),
            fltk::app::event_x_root(),
            fltk::app::event_y_root(),
            false,
            INTELLISENSE_DEFERRED_HIDE_RETRIES,
        );
    }

    fn can_try_hide_intellisense_popup(state: IntellisensePopupTransitionState) -> bool {
        matches!(state, IntellisensePopupTransitionState::Idle)
    }

    pub fn try_hide_intellisense_popup(&self) {
        if !Self::can_try_hide_intellisense_popup(self.intellisense_runtime.popup_transition_state())
        {
            return;
        }

        let mut popup = match self.intellisense_popup.try_lock() {
            Ok(popup) => popup,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
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
        let Some(connection) = self.connection_binding.metadata_connection() else {
            return;
        };
        Self::trigger_intellisense(
            &self.editor,
            &self.buffer,
            &self.highlight_shadow,
            &self.intellisense_data,
            &self.intellisense_popup,
            &self.column_sender,
            &connection,
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
        let preferred_db_type = Some(self.current_db_type());
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

        let Some(connection) = self.bound_connection() else {
            Self::show_alert_dialog("This query tab is not connected to a database");
            return;
        };
        let tab_scope = self.connection_binding.snapshot().scope;
        // The tab's own answer to "how long may my work take", read on the UI
        // thread where the input lives — the same value F6 reads.
        let query_timeout = Self::parse_timeout(&self.timeout_input.value());
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
                        tab_scope.as_deref(),
                        query_timeout,
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

    pub(super) fn describe_object_for_current_db(
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        object_name: &str,
        qualifier: Option<&str>,
        scope: Option<&str>,
        query_timeout: Option<Duration>,
    ) -> Result<QuickDescribeData, String> {
        quick_describe_backend_for(conn_guard.db_type()).describe_object(
            conn_guard,
            object_name,
            qualifier,
            scope,
            query_timeout,
        )
    }

    /// `scope` is the database the requesting query tab has selected; an
    /// unqualified name is looked up there.
    fn describe_mysql_object(
        conn: &mut mysql::Conn,
        object_name: &str,
        qualifier: Option<&str>,
        scope: Option<&str>,
    ) -> Result<QuickDescribeData, String> {
        use crate::db::query::mysql_executor::MysqlObjectBrowser;

        let object_name = Self::strip_identifier_quotes(object_name);
        let qualifier = qualifier.map(Self::strip_identifier_quotes);
        let qualifier = qualifier.as_deref();
        let qualified_name = qualifier
            .map(|schema| format!("{schema}.{}", object_name))
            .unwrap_or_else(|| object_name.clone());
        // What the name is written as stays the display name; what it is looked
        // up in follows the tab.
        let qualifier = qualifier.or(scope);

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
                    let kind = if object_type_upper == "FUNCTION" {
                        crate::db::query::mysql_executor::MysqlRoutineKind::Function
                    } else {
                        crate::db::query::mysql_executor::MysqlRoutineKind::Procedure
                    };
                    let args = MysqlObjectBrowser::get_routine_arguments_in_schema(
                        conn,
                        qualifier,
                        &object_name,
                        kind,
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
