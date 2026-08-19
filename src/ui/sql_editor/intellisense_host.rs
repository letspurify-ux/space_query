use super::*;

impl SqlEditorWidget {
    pub(crate) fn finalize_execution_state(
        query_running: &Arc<Mutex<bool>>,
        cancel_flag: &Arc<Mutex<bool>>,
    ) {
        store_mutex_bool(cancel_flag, false);
        // Publish the idle state last. Once another operation observes
        // query_running=false it may install and cancel a new operation, so
        // this cleanup must not write the shared cancel flag afterwards.
        store_mutex_bool(query_running, false);
    }

    pub(crate) fn setup_column_loader(&self, column_receiver: mpsc::Receiver<ColumnLoadUpdate>) {
        let intellisense_data = self.intellisense_data.clone();
        let editor = self.editor.clone();
        let buffer = self.buffer.clone();
        let highlighter = self.highlighter.clone();
        let widget = self.clone();
        let intellisense_popup = self.intellisense_popup.clone();
        let column_sender = self.column_sender.clone();
        let connection_binding = self.connection_binding.clone();
        let intellisense_runtime = self.intellisense_runtime.clone();

        let receiver: Arc<Mutex<mpsc::Receiver<ColumnLoadUpdate>>> =
            Arc::new(Mutex::new(column_receiver));

        const COLUMN_POLL_ACTIVE_INTERVAL_SECONDS: f64 = 0.05;
        const COLUMN_POLL_IDLE_INTERVAL_SECONDS: f64 = 0.5;
        const COLUMN_LOADING_STALE_TIMEOUT: Duration = Duration::from_secs(8);

        fn schedule_poll(
            receiver: Arc<Mutex<mpsc::Receiver<ColumnLoadUpdate>>>,
            intellisense_data: Arc<Mutex<IntellisenseData>>,
            editor: TextEditor,
            buffer: TextBuffer,
            highlighter: Arc<Mutex<SqlHighlighter>>,
            widget: SqlEditorWidget,
            intellisense_popup: Arc<Mutex<IntellisensePopup>>,
            column_sender: mpsc::Sender<ColumnLoadUpdate>,
            connection_binding: TabConnectionBinding,
            intellisense_runtime: Arc<IntellisenseRuntimeState>,
        ) {
            if editor.was_deleted() {
                return;
            }

            let mut disconnected = false;
            let mut processed = 0usize;
            let mut pending_action = ColumnPollPendingAction::None;
            let mut highlight_columns: Option<Vec<String>> = None;
            {
                let r = receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    match r.try_recv() {
                        Ok(update) => {
                            processed += 1;
                            let (new_pending_action, new_highlight_columns) = {
                                let mut data = intellisense_data
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if update.is_foreign_keys {
                                    // Foreign-key result (lazy, for JOIN auto-join). Refresh so
                                    // the auto-join suggestion / FK badge can appear; never
                                    // touches column-loading tracking or highlighting.
                                    if update.cache_columns {
                                        data.set_foreign_keys_for_table(
                                            &update.table,
                                            update.foreign_keys,
                                        );
                                        (ColumnPollPendingAction::Refresh, None)
                                    } else {
                                        data.clear_foreign_keys_loading(&update.table);
                                        (ColumnPollPendingAction::None, None)
                                    }
                                } else if update.cache_columns {
                                    data.set_column_meta_for_table(
                                        &update.table,
                                        update.column_meta,
                                    );
                                    data.set_columns_for_table(&update.table, update.columns);
                                    (
                                        ColumnPollPendingAction::Refresh,
                                        Some(collect_highlight_columns_from_intellisense(&data)),
                                    )
                                } else {
                                    data.clear_columns_loading(&update.table);
                                    (
                                        if data.columns_loading.is_empty() {
                                            ColumnPollPendingAction::Clear
                                        } else {
                                            ColumnPollPendingAction::None
                                        },
                                        None,
                                    )
                                }
                            };
                            if matches!(
                                new_pending_action,
                                ColumnPollPendingAction::Refresh
                                    | ColumnPollPendingAction::RefreshThenClear
                            ) {
                                pending_action.request_refresh();
                            }
                            if matches!(
                                new_pending_action,
                                ColumnPollPendingAction::Clear
                                    | ColumnPollPendingAction::RefreshThenClear
                            ) {
                                pending_action.request_clear();
                            }
                            if new_highlight_columns.is_some() {
                                highlight_columns = new_highlight_columns;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            if disconnected {
                return;
            }

            if let Some(highlight_columns) = highlight_columns {
                let highlight_data_changed = {
                    let mut highlighter = highlighter
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let mut highlight_data = highlighter.get_highlight_data();
                    if highlight_data.columns == highlight_columns {
                        false
                    } else {
                        highlight_data.columns = highlight_columns;
                        highlighter.set_highlight_data(highlight_data);
                        true
                    }
                };

                if highlight_data_changed {
                    widget.schedule_deferred_visible_semantic_rehighlight();
                }
            }

            if pending_action.should_refresh() {
                let pending = intellisense_runtime.pending_intellisense();
                if let Some(pending) = pending {
                    let (cursor_pos, _) = SqlEditorWidget::editor_cursor_position(&editor, &buffer);
                    if cursor_pos == pending.cursor_pos {
                        if let Some(connection) = connection_binding.metadata_connection() {
                            SqlEditorWidget::trigger_intellisense(
                                &editor,
                                &buffer,
                                &widget.highlight_shadow,
                                &intellisense_data,
                                &intellisense_popup,
                                &column_sender,
                                &connection,
                                &intellisense_runtime,
                            );
                        }
                    } else {
                        intellisense_runtime.clear_pending_intellisense();
                    }
                }
            }

            if matches!(
                pending_action,
                ColumnPollPendingAction::Clear | ColumnPollPendingAction::RefreshThenClear
            ) {
                let has_columns_loading = {
                    let data = intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    !data.columns_loading.is_empty()
                };
                if pending_action.should_clear(has_columns_loading) {
                    intellisense_runtime.clear_pending_intellisense();
                }
            }

            let stale_cleared = {
                let mut data = intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                data.clear_stale_columns_loading(COLUMN_LOADING_STALE_TIMEOUT)
            };
            if stale_cleared > 0 {
                processed += stale_cleared;
                let no_columns_loading = {
                    let data = intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    data.columns_loading.is_empty()
                };
                if no_columns_loading {
                    intellisense_runtime.clear_pending_intellisense();
                }
            }

            let has_pending_column_work =
                {
                    let data = intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    !data.columns_loading.is_empty()
                } || intellisense_runtime.pending_intellisense().is_some();

            let delay = if processed > 0 {
                0.0
            } else if has_pending_column_work {
                COLUMN_POLL_ACTIVE_INTERVAL_SECONDS
            } else {
                COLUMN_POLL_IDLE_INTERVAL_SECONDS
            };

            crate::ui::ui_timeout::schedule(delay, move || {
                schedule_poll(
                    receiver.clone(),
                    intellisense_data.clone(),
                    editor.clone(),
                    buffer.clone(),
                    highlighter.clone(),
                    widget.clone(),
                    intellisense_popup.clone(),
                    column_sender.clone(),
                    connection_binding.clone(),
                    intellisense_runtime.clone(),
                );
            });
        }

        schedule_poll(
            receiver,
            intellisense_data,
            editor,
            buffer,
            highlighter,
            widget,
            intellisense_popup,
            column_sender,
            connection_binding,
            intellisense_runtime,
        );
    }

    pub(crate) fn setup_syntax_highlighting(&self) {
        let mut buffer = self.buffer.clone();
        let widget = self.clone();
        let intellisense_runtime = self.intellisense_runtime.clone();
        let undo_state = self.undo_redo_state.clone();
        let applying_history_navigation = self.applying_history_navigation.clone();
        let suppress_buffer_callbacks = self.suppress_buffer_callbacks.clone();
        let pending_paste_text = self.pending_paste_text.clone();
        buffer.add_modify_callback2(move |buf, pos, ins, del, _restyled, deleted_text| {
            if ins <= 0 && del <= 0 {
                return;
            }
            crate::ui::sql_editor::ime_trace(|| {
                format!(
                    "modify pos={pos} ins={ins} del={del} deleted_text={deleted_text:?} \
                     compose_state={} selection={:?}",
                    fltk::app::compose_state(),
                    buf.selection_position(),
                )
            });
            if load_mutex_bool(&suppress_buffer_callbacks) {
                return;
            }
            let shared_inserted_text =
                take_matching_pending_paste_text(buf, pos, ins, &pending_paste_text)
                    .unwrap_or_else(|| Arc::new(inserted_text(buf, pos, ins)));
            let edit = BufferEdit {
                start: pos.max(0) as usize,
                deleted_len: del.max(0) as usize,
                inserted_text: shared_inserted_text,
            };

            let is_applying_navigation = *applying_history_navigation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let updated_text_snapshot = if is_applying_navigation {
                None
            } else {
                let mut state = undo_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.applying_history {
                    state.pending_history_text_snapshots.pop_front()
                } else {
                    let edit_group =
                        classify_edit_group(ins, del, edit.inserted_text.as_str(), deleted_text);
                    Some(state.record_edit(&edit, edit_group))
                }
            };

            intellisense_runtime.apply_buffer_edit(
                pos.max(0) as usize,
                ins.max(0) as usize,
                del.max(0) as usize,
            );
            widget.handle_buffer_highlight_update_with_known_inserted_text(
                buf,
                pos,
                ins,
                del,
                edit.inserted_text.as_str(),
                deleted_text,
                updated_text_snapshot.as_ref(),
            );
        });
    }

    pub fn cleanup_for_close(&mut self) {
        let query_was_running = self.is_query_running();
        // A statement this tab accepted but never started must not start into a
        // tab that no longer exists. The close road already asks the cancel
        // road first; this is the backstop for every other way a tab goes away.
        self.abandon_deferred_executions();
        self.cancel_active_lazy_fetch(false);
        // Close, not clear: a statement that outlives this tab (a cancel that
        // never landed) hands its session back later, and a closed slot is
        // what makes that hand-back close the session instead of retaining it
        // into a tab that no longer exists.
        self.pooled_db_session.close_for_owner_shutdown();
        if !query_was_running {
            *self
                .current_operation_cancel_handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            Self::finalize_execution_state(&self.query_running, &self.cancel_flag);
            Self::set_current_query_connection(
                &self.current_query_connection,
                &self.current_query_cancel_handle,
                None,
            );
            Self::set_current_mysql_cancel_context(
                &self.current_mysql_cancel_context,
                &self.current_query_cancel_handle,
                None,
            );
        }

        *self
            .execute_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .progress_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .status_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .find_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .replace_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .file_drop_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;

        Self::invalidate_keyup_debounce(&self.intellisense_runtime);
        self.deferred_semantic_rehighlight_generation
            .fetch_add(1, Ordering::Relaxed);
        if let Some(handle) = self
            .deferred_semantic_rehighlight_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            crate::ui::ui_timeout::cancel(handle);
        }
        self.intellisense_runtime.next_parse_generation();
        self.intellisense_runtime
            .set_popup_transition_state(IntellisensePopupTransitionState::Idle);

        self.intellisense_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .delete_for_close();
        self.delete_signature_popup_for_close();
        self.highlighter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_highlight_data(HighlightData::new());
        self.highlight_shadow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        {
            let _suppress_callbacks = self.suppress_buffer_callbacks();
            self.buffer.set_text("");
            self.style_buffer.set_text("");
        }
        self.intellisense_runtime.clear_ui_tracking();
        self.intellisense_runtime.clear_parse_cache();
        self.intellisense_runtime.clear_routine_symbol_cache();
        self.history_cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        self.history_original
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        self.history_navigation_entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *self
            .applying_history_navigation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Self::reset_word_undo_state(&self.undo_redo_state);
    }
}
