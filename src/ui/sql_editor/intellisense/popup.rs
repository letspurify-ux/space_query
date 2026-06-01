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
    match db_type.backend_kind() {
        crate::db::DatabaseBackendKind::Oracle => &ORACLE_QUICK_DESCRIBE_BACKEND,
        crate::db::DatabaseBackendKind::MySql => &MYSQL_QUICK_DESCRIBE_BACKEND,
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
                crate::db::DatabaseConnection::apply_oracle_thin_current_schema(
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

trait SignatureBackend: Sync {
    /// Resolve the argument list of a routine call, trying package-member then
    /// standalone resolution. Returns `None` when no such routine is found.
    fn resolve(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        name: &str,
        qualifier: Option<&str>,
    ) -> Option<Vec<ProcedureArgument>>;
}

struct OracleSignatureBackend;
struct MysqlSignatureBackend;

static ORACLE_SIGNATURE_BACKEND: OracleSignatureBackend = OracleSignatureBackend;
static MYSQL_SIGNATURE_BACKEND: MysqlSignatureBackend = MysqlSignatureBackend;

fn signature_backend_for(db_type: crate::db::DatabaseType) -> &'static dyn SignatureBackend {
    match db_type.backend_kind() {
        crate::db::DatabaseBackendKind::Oracle => &ORACLE_SIGNATURE_BACKEND,
        crate::db::DatabaseBackendKind::MySql => &MYSQL_SIGNATURE_BACKEND,
    }
}

fn non_empty(args: Vec<ProcedureArgument>) -> Option<Vec<ProcedureArgument>> {
    (!args.is_empty()).then_some(args)
}

impl SignatureBackend for OracleSignatureBackend {
    fn resolve(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        name: &str,
        qualifier: Option<&str>,
    ) -> Option<Vec<ProcedureArgument>> {
        let tracked_schema = conn_guard
            .tracked_oracle_current_schema()
            .map(str::to_string);
        match conn_guard.require_live_db_connection().ok()? {
            crate::db::DbConnection::Oracle(db_conn) => {
                let conn = db_conn.as_ref();
                if let Some(qualifier) = qualifier {
                    if let Some(args) = ObjectBrowser::get_package_procedure_arguments(
                        conn, qualifier, name,
                    )
                    .ok()
                    .and_then(non_empty)
                    {
                        return Some(args);
                    }
                    let qualified = format!("{qualifier}.{name}");
                    return ObjectBrowser::get_procedure_arguments(conn, &qualified)
                        .ok()
                        .and_then(non_empty);
                }
                ObjectBrowser::get_procedure_arguments(conn, name)
                    .ok()
                    .and_then(non_empty)
            }
            crate::db::DbConnection::OracleThin(db_conn) => {
                let mut session = db_conn.lock().ok()?;
                let _ = crate::db::DatabaseConnection::apply_oracle_thin_current_schema(
                    &mut session,
                    tracked_schema.as_deref(),
                );
                if let Some(qualifier) = qualifier {
                    if let Some(args) = ObjectBrowser::get_thin_package_procedure_arguments(
                        &mut session,
                        qualifier,
                        name,
                    )
                    .ok()
                    .and_then(non_empty)
                    {
                        return Some(args);
                    }
                    let qualified = format!("{qualifier}.{name}");
                    return ObjectBrowser::get_thin_procedure_arguments(&mut session, &qualified)
                        .ok()
                        .and_then(non_empty);
                }
                ObjectBrowser::get_thin_procedure_arguments(&mut session, name)
                    .ok()
                    .and_then(non_empty)
            }
            crate::db::DbConnection::MySQL { .. } => None,
        }
    }
}

impl SignatureBackend for MysqlSignatureBackend {
    fn resolve(
        &self,
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        name: &str,
        qualifier: Option<&str>,
    ) -> Option<Vec<ProcedureArgument>> {
        let _ = conn_guard.apply_tracked_mysql_current_database();
        let conn = conn_guard.get_mysql_connection_mut()?;
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_routine_arguments_in_schema(
            conn, qualifier, name,
        )
        .ok()
        .and_then(non_empty)
    }
}

impl SqlEditorWidget {
    /// Lookup key for a routine call: `QUALIFIER.NAME` uppercased.
    fn signature_key(call: &crate::ui::intellisense::EnclosingCall) -> String {
        match &call.qualifier {
            Some(qualifier) => format!("{}.{}", qualifier.to_uppercase(), call.name.to_uppercase()),
            None => call.name.to_uppercase(),
        }
    }

    fn resolve_signature_label(
        conn_guard: &mut crate::db::ConnectionLockGuard<'_>,
        name: &str,
        qualifier: Option<&str>,
    ) -> Option<SignatureLabel> {
        let db_type = conn_guard.db_type();
        let args = signature_backend_for(db_type).resolve(conn_guard, name, qualifier)?;
        Some(Self::build_signature_label(name, &args))
    }

    /// Screen position for the signature popup: just above the line containing
    /// the call's opening parenthesis.
    fn signature_popup_position(editor: &TextEditor, anchor_pos: i32) -> (i32, i32) {
        let (cursor_x, cursor_y) = editor.position_to_xy(anchor_pos);
        let (win_x, win_y) = editor
            .window()
            .map(|win| (win.x_root(), win.y_root()))
            .unwrap_or((0, 0));
        let x = win_x + cursor_x;
        let y = (win_y + cursor_y - SignaturePopup::height() - 2).max(win_y);
        (x, y)
    }

    pub(crate) fn signature_popup_is_visible(&self) -> bool {
        self.signature_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_visible()
    }

    fn hide_signature_popup(&self) {
        self.signature_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
    }

    /// Recompute the routine call enclosing the cursor and show, hide, or fetch
    /// its signature accordingly. Cheap on cache hits; spawns a background
    /// fetch (deduplicated) on a miss.
    pub(crate) fn update_signature_hint(&self) {
        if self.editor.was_deleted() {
            return;
        }
        let cursor = self.editor.insert_position().max(0) as usize;

        // Scan only a bounded window before the cursor (snapped to a line
        // start) instead of cloning the whole buffer on every keystroke,
        // matching the editor's deliberately lightweight KeyUp handling. A
        // call's opening parenthesis is effectively always within this window.
        const SIGNATURE_SCAN_WINDOW: usize = 4000;
        let window_start = cursor.saturating_sub(SIGNATURE_SCAN_WINDOW);
        let raw = self
            .buffer
            .text_range(window_start as i32, cursor as i32)
            .unwrap_or_default();
        let (scan_offset, scan_text) = match raw.find('\n') {
            Some(newline) if window_start > 0 => {
                (window_start + newline + 1, raw[newline + 1..].to_string())
            }
            _ => (window_start, raw),
        };

        let Some(mut call) =
            crate::ui::intellisense::enclosing_call_at_cursor(&scan_text, scan_text.len())
        else {
            self.hide_signature_popup();
            return;
        };
        call.open_paren += scan_offset;

        // Built-in functions have no data-dictionary argument rows; skip the
        // futile lookup instead of issuing (and caching) an empty fetch.
        if call.qualifier.is_none() && crate::ui::intellisense::is_builtin_function(&call.name) {
            self.hide_signature_popup();
            return;
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
                let (x, y) = Self::signature_popup_position(&self.editor, call.open_paren as i32);
                self.signature_popup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .show(&label, active_arg, x, y);
            }
            Action::Hide => self.hide_signature_popup(),
            Action::Fetch => {
                self.hide_signature_popup();
                self.spawn_signature_fetch(key, call.name, call.qualifier);
            }
        }
    }

    fn spawn_signature_fetch(&self, key: String, name: String, qualifier: Option<String>) {
        let connection = self.connection.clone();
        let sender = self.ui_action_sender.clone();
        let key_fallback = key.clone();
        let spawn_result = thread::Builder::new()
            .name("signature-hint".to_string())
            .spawn(move || {
                let sender_for_panic = sender.clone();
                let key_for_panic = key.clone();
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    let Some(mut conn_guard) = crate::db::try_lock_connection_with_activity(
                        &connection,
                        format!("Signature hint {}", name),
                    ) else {
                        let _ = sender.send(UiActionResult::SignatureArguments {
                            key: key.clone(),
                            label: None,
                            cache: false,
                        });
                        app::awake();
                        return;
                    };
                    let label =
                        Self::resolve_signature_label(&mut conn_guard, &name, qualifier.as_deref());
                    let _ = sender.send(UiActionResult::SignatureArguments {
                        key: key.clone(),
                        label,
                        cache: true,
                    });
                    app::awake();
                }));
                if result.is_err() {
                    let _ = sender_for_panic.send(UiActionResult::SignatureArguments {
                        key: key_for_panic,
                        label: None,
                        cache: false,
                    });
                    app::awake();
                }
            });

        if spawn_result.is_err() {
            let _ = self.ui_action_sender.send(UiActionResult::SignatureArguments {
                key: key_fallback,
                label: None,
                cache: false,
            });
            app::awake();
        }
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
    pub fn hide_intellisense_if_outside(&self, x: i32, y: i32) {
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
        let qualifier =
            Self::qualifier_before_word(&self.buffer, &self.highlight_shadow, start as usize);
        let raw_qualifier =
            Self::raw_qualifier_before_word(&self.buffer, &self.highlight_shadow, start as usize);
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
