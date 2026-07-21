impl SqlEditorWidget {
    fn should_suppress_ctrl_enter_dispatch(
        suppression: &mut EnterKeyupSuppression,
        now: std::time::Instant,
    ) -> bool {
        if matches!(
            *suppression,
            EnterKeyupSuppression::CtrlEnterExecute(previous)
                if now.saturating_duration_since(previous) < CTRL_ENTER_DUPLICATE_WINDOW
        ) {
            return true;
        }

        *suppression = EnterKeyupSuppression::CtrlEnterExecute(now);
        false
    }

    fn take_enter_keyup_suppression(
        key: Key,
        suppression: &mut EnterKeyupSuppression,
    ) -> bool {
        if !matches!(key, Key::Enter | Key::KPEnter)
            || matches!(*suppression, EnterKeyupSuppression::None)
        {
            return false;
        }

        *suppression = EnterKeyupSuppression::None;
        true
    }

    /// Apply a merge produced by [`crate::ui::sql_editor::hangul_repair`] for
    /// the macOS broken-first-Hangul-key bug.
    #[cfg(target_os = "macos")]
    fn apply_hangul_first_key_repair_edit(
        buffer: &mut TextBuffer,
        edit: Option<crate::ui::sql_editor::hangul_repair::RepairEdit>,
    ) {
        if let Some(edit) = edit {
            crate::ui::sql_editor::ime_trace(|| format!("hangul repair merge {edit:?}"));
            buffer.replace(edit.start as i32, edit.end as i32, &edit.replacement);
        }
    }

    /// Swallow duplicate navigation/editing-key dispatches caused by a
    /// pending IME composition on macOS.
    ///
    /// With a syllable still composing, one physical arrow or BackSpace press
    /// reaches the editor as a burst of FL_KEYBOARD dispatches (commit/marked
    /// dispatch with `compose_state>0`, then duplicate action dispatches —
    /// FLTK's cocoa layer processes the event through both
    /// performKeyEquivalent and keyDown; the bare-TextEditor probe shows 2x,
    /// the app 4x). Arrows moved 4 lines per press; BackSpace while composing
    /// ㅎ deleted the composing jamo AND the preceding newline. The burst
    /// arrives within well under a millisecond, while even the fastest key
    /// auto-repeat is ~15ms apart, so a same-key dispatch inside a 5ms window
    /// cannot be a real keystroke. The first dispatch of the burst passes
    /// through and performs the single move/delete; the rest of the burst is
    /// consumed.
    #[cfg(target_os = "macos")]
    fn nav_key_is_duplicate_dispatch(
        slot: &Arc<Mutex<Option<(Key, std::time::Instant)>>>,
        key: Key,
    ) -> bool {
        const BURST_WINDOW: std::time::Duration = std::time::Duration::from_millis(5);
        if !matches!(
            key,
            Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::BackSpace
                | Key::Delete
        ) {
            return false;
        }
        let now = std::time::Instant::now();
        let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((previous_key, anchored_at)) = *guard {
            if previous_key == key && now.duration_since(anchored_at) < BURST_WINDOW {
                return true;
            }
        }
        *guard = Some((key, now));
        false
    }

    /// Ask the IME to drop a pending composition, after the current event
    /// finishes dispatching.
    ///
    /// Needed for events that bypass `[[view inputContext] handleEvent:]` —
    /// Cmd/Ctrl shortcuts (FLTK's performKeyEquivalent zeroes only
    /// `Fl::compose_state`) and mouse clicks — where the IME otherwise still
    /// believes it is composing and commits the stale syllable into the next
    /// keystroke (홍길동 → Cmd+A → retype = 동홍길동). Events the IME does
    /// see (plain arrows, etc.) must NOT trigger this: the IME finalizes the
    /// composition itself, and calling discardMarkedText from inside its own
    /// event processing makes it re-dispatch the key (arrows jumped 4 lines).
    /// Deferring to a zero-delay timeout keeps the call out of any in-flight
    /// dispatch; it still runs before the next keystroke can arrive.
    #[cfg(target_os = "macos")]
    fn schedule_macos_ime_discard(editor: &TextEditor) {
        let Some(window) = editor.window() else {
            return;
        };
        crate::ui::ui_timeout::schedule(0.0, move || {
            if window.was_deleted() || !window.shown() {
                return;
            }
            crate::ui::sql_editor::ime_trace(|| "deferred discardMarkedText".to_string());
            crate::ui::sql_editor::macos_ime::discard_marked_text(
                window.raw_handle() as *mut _,
            );
        });
    }

    #[cfg(target_os = "macos")]
    fn finish_macos_ime_before_cursor_or_selection_change(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        repair_state: &Arc<Mutex<crate::ui::sql_editor::hangul_repair::FirstKeyRepairState>>,
    ) {
        fltk::draw::reset_spot();
        let caret = editor.insert_position().max(0) as usize;
        let reader = buffer.clone();
        let edit = repair_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .flush(caret, &|start, end| {
                reader.text_range(start as i32, end as i32)
            });
        if let Some(edit) = edit {
            let old_len = edit.end.saturating_sub(edit.start);
            let new_len = edit.replacement.len();
            crate::ui::sql_editor::ime_trace(|| format!("hangul repair merge {edit:?}"));
            buffer.replace(edit.start as i32, edit.end as i32, &edit.replacement);
            if caret >= edit.end {
                let new_caret = caret.saturating_sub(old_len).saturating_add(new_len);
                editor.set_insert_position(new_caret.min(i32::MAX as usize) as i32);
            } else if caret > edit.start {
                editor.set_insert_position((edit.start + new_len).min(i32::MAX as usize) as i32);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn marked_text_replaced_by_user_selection(
        buffer: &TextBuffer,
        selection: (i32, i32),
        caret: i32,
        compose_state: i32,
    ) -> String {
        if compose_state <= 0 {
            return String::new();
        }

        let (selection_start, selection_end) = if selection.0 <= selection.1 {
            selection
        } else {
            (selection.1, selection.0)
        };
        let compose_len = compose_state;
        let candidates = [
            caret
                .checked_sub(compose_len)
                .map(|start| (start, caret)),
            selection_end
                .checked_sub(compose_len)
                .map(|start| (start, selection_end)),
        ];

        for range in candidates.into_iter().flatten() {
            let (start, end) = range;
            if start >= selection_start && end <= selection_end && start < end {
                return buffer.text_range(start, end).unwrap_or_default();
            }
        }

        String::new()
    }

    #[cfg(target_os = "macos")]
    fn remember_macos_ime_marked_text(
        editor: &TextEditor,
        buffer: &TextBuffer,
        marked_text: &Arc<Mutex<String>>,
    ) {
        let selection = buffer.selection_position();
        if !Self::selection_is_current_ime_marked_range(
            selection,
            editor.insert_position(),
            fltk::app::compose_state(),
        ) {
            return;
        }

        let Some((raw_start, raw_end)) = selection else {
            return;
        };
        let (start, end) = if raw_start <= raw_end {
            (raw_start, raw_end)
        } else {
            (raw_end, raw_start)
        };
        *marked_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            buffer.text_range(start, end).unwrap_or_default();
    }

    #[cfg(target_os = "macos")]
    fn marked_text_snapshot_in_selection(
        buffer: &TextBuffer,
        selection: (i32, i32),
        marked_text: &Arc<Mutex<String>>,
    ) -> String {
        let marked = marked_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if marked.is_empty() {
            return String::new();
        }

        let (selection_start, selection_end) = if selection.0 <= selection.1 {
            selection
        } else {
            (selection.1, selection.0)
        };
        let Ok(marked_len) = i32::try_from(marked.len()) else {
            return String::new();
        };
        let Some(marked_start) = selection_end.checked_sub(marked_len) else {
            return String::new();
        };
        if marked_start < selection_start {
            return String::new();
        }

        if buffer
            .text_range(marked_start, selection_end)
            .as_deref()
            == Some(marked.as_str())
        {
            marked
        } else {
            String::new()
        }
    }

    #[cfg(target_os = "macos")]
    fn handle_macos_user_selection_text_input(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        repair_state: &Arc<Mutex<crate::ui::sql_editor::hangul_repair::FirstKeyRepairState>>,
        marked_text: &Arc<Mutex<String>>,
    ) -> bool {
        let key = fltk::app::event_key();
        if matches!(key, Key::Enter | Key::KPEnter | Key::Tab) {
            return false;
        }

        let state = fltk::app::event_state();
        if state.contains(fltk::enums::Shortcut::Ctrl)
            || state.contains(fltk::enums::Shortcut::Command)
            || state.contains(fltk::enums::Shortcut::Alt)
        {
            return false;
        }

        let event_text = fltk::app::event_text();
        if event_text.is_empty() {
            return false;
        }

        let selection = buffer.selection_position();
        if !Self::selection_is_user_replacement_range(
            selection,
            editor.insert_position(),
            fltk::app::compose_state(),
        ) {
            return false;
        }
        let Some((raw_start, raw_end)) = selection else {
            return false;
        };
        let (selection_start, selection_end) = if raw_start <= raw_end {
            (raw_start, raw_end)
        } else {
            (raw_end, raw_start)
        };

        let mut replaced_marked_text = Self::marked_text_replaced_by_user_selection(
            buffer,
            (selection_start, selection_end),
            editor.insert_position(),
            fltk::app::compose_state(),
        );
        if replaced_marked_text.is_empty() {
            replaced_marked_text = Self::marked_text_snapshot_in_selection(
                buffer,
                (selection_start, selection_end),
                marked_text,
            );
        }
        if fltk::app::compose().is_none() {
            crate::ui::sql_editor::ime_trace(|| {
                "selection-input: compose()=None, falling through".to_string()
            });
            return false;
        }

        let inserted =
            Self::ime_user_selection_replacement_text(&event_text, &replaced_marked_text);
        crate::ui::sql_editor::ime_trace(|| {
            format!(
                "selection-input: sel=({selection_start},{selection_end}) event={event_text:?} \
                 replaced_marked={replaced_marked_text:?} inserted={inserted:?}"
            )
        });
        buffer.replace(selection_start, selection_end, &inserted);
        let cursor = selection_start
            .max(0)
            .saturating_add(inserted.len().min(i32::MAX as usize) as i32);
        editor.set_insert_position(cursor);

        let compose_len = fltk::app::compose_state().max(0);
        if compose_len > 0 && compose_len <= inserted.len().min(i32::MAX as usize) as i32 {
            buffer.select(cursor - compose_len, cursor);
        } else {
            buffer.unselect();
        }
        editor.show_insert_position();
        repair_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reset();
        marked_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        true
    }

    fn invoke_context_action_callback(
        callback: &SqlEditorContextActionCallback,
        action: SqlEditorContextAction,
    ) {
        Self::invoke_callback(callback, "editor context action callback", move |callback_fn| {
            callback_fn(action)
        });
    }

    fn schedule_context_action_callback(
        callback: &SqlEditorContextActionCallback,
        action: SqlEditorContextAction,
    ) {
        let callback = callback.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            Self::invoke_context_action_callback(&callback, action);
        });
    }

    fn show_editor_context_menu(
        editor: &mut TextEditor,
        context_action_callback: &SqlEditorContextActionCallback,
    ) {
        let mouse_x = app::event_x();
        let mouse_y = app::event_y();
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut menu = MenuButton::new(mouse_x, mouse_y, 0, 0, None);
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        menu.add_choice("Close|Close All|Cut|Copy|Paste");

        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        if let Some(choice) = menu.popup() {
            let choice_label = choice.label().unwrap_or_default();
            match choice_label.as_str() {
                "Cut" => {
                    editor.cut();
                }
                "Copy" => {
                    editor.copy();
                }
                "Paste" => {
                    editor.paste();
                }
                "Close" => {
                    Self::schedule_context_action_callback(
                        context_action_callback,
                        SqlEditorContextAction::Close,
                    );
                }
                "Close All" => {
                    Self::schedule_context_action_callback(
                        context_action_callback,
                        SqlEditorContextAction::CloseAll,
                    );
                }
                _ => {}
            }
        }

        MenuButton::delete(menu);
    }

    pub fn setup_intellisense(&mut self) {
        let buffer = self.buffer.clone();
        let mut editor = self.editor.clone();
        let intellisense_data = self.intellisense_data.clone();
        let intellisense_popup = self.intellisense_popup.clone();
        let connection = self.connection.clone();
        let column_sender = self.column_sender.clone();
        let text_shadow = self.highlight_shadow.clone();
        let enter_keyup_suppression = Arc::new(Mutex::new(EnterKeyupSuppression::None));
        let navigation_keyup_state = Arc::new(Mutex::new(NavigationKeyupState::Idle));
        let intellisense_runtime = self.intellisense_runtime.clone();

        // Setup callback for inserting selected text
        let mut buffer_for_insert = buffer.clone();
        let mut editor_for_insert = editor.clone();
        let intellisense_runtime_for_insert = intellisense_runtime.clone();
        let intellisense_data_for_insert = intellisense_data.clone();
        let column_sender_for_insert = column_sender.clone();
        let connection_for_insert = connection.clone();
        let text_shadow_for_insert = text_shadow.clone();
        let preferred_insert_position_for_insert = self.preferred_insert_position.clone();
        let undo_redo_state_for_insert = self.undo_redo_state.clone();
        let widget_for_insert = self.clone();
        {
            let mut popup = intellisense_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            popup.set_selected_callback(move |selected| {
                let (cursor_pos, cursor_pos_usize) =
                    Self::editor_cursor_position(&editor_for_insert, &buffer_for_insert);
                let preferred_db_type = Some(
                    intellisense_runtime_for_insert
                        .db_type_without_blocking(&connection_for_insert),
                );
                let context_text =
                    Self::normalize_intellisense_context_text(&Self::context_before_cursor(
                        &buffer_for_insert,
                        &text_shadow_for_insert,
                        cursor_pos,
                        preferred_db_type,
                        intellisense_runtime_for_insert.context_window_bytes(),
                    ));
                let context = detect_sql_context(&context_text, context_text.len());
                if matches!(context, SqlContext::TableName) {
                    let (_, word_start, _) = Self::word_at_cursor_for_db(
                        &buffer_for_insert,
                        &text_shadow_for_insert,
                        cursor_pos,
                        preferred_db_type,
                    );
                    let qualifier = Self::qualifier_before_word(
                        &buffer_for_insert,
                        &text_shadow_for_insert,
                        word_start,
                        preferred_db_type,
                    );
                    let table_lookup = qualifier
                        .as_deref()
                        .map(|qualifier| format!("{}.{}", qualifier, selected))
                        .unwrap_or_else(|| selected.clone());
                    let should_prefetch = {
                        let data = intellisense_data_for_insert
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        Self::resolve_table_column_load_key(&data, &table_lookup).is_some()
                    };
                    if should_prefetch {
                        Self::request_table_columns_for_db(
                            &table_lookup,
                            &intellisense_data_for_insert,
                            &column_sender_for_insert,
                            &connection_for_insert,
                            preferred_db_type,
                        );
                    }
                }
                let range = intellisense_runtime_for_insert.completion_range();
                let (start, end) = Self::completion_replacement_range(
                    &buffer_for_insert,
                    &text_shadow_for_insert,
                    cursor_pos,
                    range,
                );

                let inserted = Self::completion_insert_text(&selected);
                let caret_offset = Self::completion_caret_offset(&inserted);
                let completion_changes_text =
                    Self::completion_changes_text(&buffer_for_insert, start, end, &inserted);
                {
                    let mut undo_state = undo_redo_state_for_insert
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if completion_changes_text {
                        undo_state.record_cursor_move_to_if_remote(cursor_pos_usize);
                    }
                    undo_state.sync_current_cursor(cursor_pos_usize);
                    if completion_changes_text {
                        undo_state.prepare_completion_edit();
                    } else {
                        undo_state.finish_active_group();
                    }
                }
                if start != end {
                    buffer_for_insert.replace(start as i32, end as i32, &inserted);
                    editor_for_insert.set_insert_position((start + caret_offset) as i32);
                } else {
                    buffer_for_insert.insert(cursor_pos, &inserted);
                    editor_for_insert
                        .set_insert_position((cursor_pos_usize + caret_offset) as i32);
                }
                let after_cursor = if start != end {
                    start.saturating_add(caret_offset)
                } else {
                    cursor_pos_usize.saturating_add(caret_offset)
                };
                undo_redo_state_for_insert
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .finish_completion_edit_cursor(after_cursor, completion_changes_text);
                Self::sync_preferred_insert_position_from_editor(
                    &preferred_insert_position_for_insert,
                    &editor_for_insert,
                    &buffer_for_insert,
                );
                Self::finalize_completion_after_selection(&intellisense_runtime_for_insert);
                widget_for_insert.schedule_signature_hint_update();
            });
        }

        // Handle keyboard events for triggering intellisense and syntax highlighting
        let mut buffer_for_handle = buffer;
        let intellisense_data_for_handle = intellisense_data;
        let intellisense_popup_for_handle = intellisense_popup;
        let column_sender_for_handle = column_sender;
        let connection_for_handle = connection;
        let enter_keyup_suppression_for_handle = enter_keyup_suppression;
        let navigation_keyup_state_for_handle = navigation_keyup_state;
        let intellisense_runtime_for_handle = intellisense_runtime;
        let text_shadow_for_handle = text_shadow;
        let mut keydown_buffer_revision_for_handle =
            intellisense_runtime_for_handle.current_buffer_revision();
        let mut keydown_had_ctrl_or_cmd_for_handle = false;
        let mut keydown_had_alt_for_handle = false;
        let mut widget_for_shortcuts = self.clone();
        let find_callback_for_handle = self.find_callback.clone();
        let replace_callback_for_handle = self.replace_callback.clone();
        let file_drop_callback_for_handle = self.file_drop_callback.clone();
        let object_context_callback_for_handle = self.object_context_callback.clone();
        let context_action_callback_for_handle = self.context_action_callback.clone();
        let dnd_drop_state_for_handle = Arc::new(Mutex::new(DndDropState::Idle));
        let dnd_scroll_origin_for_handle = Arc::new(Mutex::new(None::<(i32, i32)>));
        let preferred_insert_position_for_handle = self.preferred_insert_position.clone();
        let undo_redo_state_for_handle = self.undo_redo_state.clone();
        let pending_paste_text_for_handle = self.pending_paste_text.clone();
        let display_metrics_ready_for_handle = self.display_metrics_ready.clone();
        #[cfg(target_os = "macos")]
        let hangul_repair_for_handle = Arc::new(Mutex::new(
            crate::ui::sql_editor::hangul_repair::FirstKeyRepairState::default(),
        ));
        #[cfg(target_os = "macos")]
        let macos_ime_marked_text_for_handle = Arc::new(Mutex::new(String::new()));
        #[cfg(target_os = "macos")]
        let nav_key_dedupe_for_handle: Arc<Mutex<Option<(Key, std::time::Instant)>>> =
            Arc::new(Mutex::new(None));

        editor.handle(move |ed, ev| {
            if ev != Event::Paste {
                pending_paste_text_for_handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
            }
            if Self::should_consume_pointer_event_until_display_metrics_ready(
                display_metrics_ready_for_handle.load(Ordering::Acquire),
                ev,
            ) {
                return true;
            }

            match ev {
                Event::DndEnter => {
                    let scroll_origin =
                        Self::set_dnd_scroll_origin(&dnd_scroll_origin_for_handle, ed);
                    Self::set_dnd_drop_target_active(ed, true);
                    let position = Self::move_editor_cursor_to_dnd_drop_position(
                        ed,
                        &buffer_for_handle,
                        &preferred_insert_position_for_handle,
                        Some(scroll_origin),
                    );
                    Self::set_dnd_drop_state(
                        &dnd_drop_state_for_handle,
                        DndDropState::AwaitingPaste(PendingDndDrop { position }),
                    );
                    true
                }
                Event::DndDrag => {
                    let scroll_origin =
                        Self::dnd_scroll_origin(&dnd_scroll_origin_for_handle, ed);
                    let position = Self::move_editor_cursor_to_dnd_drop_position(
                        ed,
                        &buffer_for_handle,
                        &preferred_insert_position_for_handle,
                        Some(scroll_origin),
                    );
                    Self::set_dnd_drop_state(
                        &dnd_drop_state_for_handle,
                        DndDropState::AwaitingPaste(PendingDndDrop { position }),
                    );
                    true
                }
                Event::DndLeave => {
                    Self::set_dnd_drop_state(&dnd_drop_state_for_handle, DndDropState::Idle);
                    Self::set_dnd_drop_target_active(ed, false);
                    Self::restore_editor_scroll(
                        ed,
                        Self::take_dnd_scroll_origin(&dnd_scroll_origin_for_handle),
                    );
                    true
                }
                Event::DndRelease => {
                    let scroll_origin =
                        Self::dnd_scroll_origin(&dnd_scroll_origin_for_handle, ed);
                    let position = Self::move_editor_cursor_to_dnd_drop_position(
                        ed,
                        &buffer_for_handle,
                        &preferred_insert_position_for_handle,
                        Some(scroll_origin),
                    );
                    Self::set_dnd_drop_state(
                        &dnd_drop_state_for_handle,
                        DndDropState::AwaitingPaste(PendingDndDrop { position }),
                    );
                    Self::set_dnd_drop_target_active(ed, false);
                    true
                }
                Event::Enter | Event::Move | Event::Drag | Event::Released => {
                    if ev == Event::Released {
                        widget_for_shortcuts.schedule_deferred_visible_semantic_rehighlight();
                    }
                    // Drag-and-drop only needs the eventual Paste payload.
                    // Avoid cursor hit-testing while the editor is in FLTK's DnD
                    // sequence because that path is unrelated to payload handling
                    // and can trip widget-internal geometry assumptions.
                    if Self::should_skip_pointer_position_tracking(&dnd_drop_state_for_handle) {
                        return false;
                    }
                    let pos = ed.xy_to_position(
                        fltk::app::event_x(),
                        fltk::app::event_y(),
                        PositionType::Cursor,
                    );
                    if pos >= 0 {
                        Self::remember_preferred_insert_position(
                            &preferred_insert_position_for_handle,
                            &buffer_for_handle,
                            pos,
                        );
                    } else {
                        Self::sync_preferred_insert_position_from_editor(
                            &preferred_insert_position_for_handle,
                            ed,
                            &buffer_for_handle,
                        );
                    }
                    false
                }
                Event::MouseWheel => {
                    widget_for_shortcuts.schedule_deferred_visible_semantic_rehighlight();
                    false
                }
                Event::Push => {
                    widget_for_shortcuts.hide_signature_popup();
                    #[cfg(target_os = "macos")]
                    {
                        Self::finish_macos_ime_before_cursor_or_selection_change(
                            ed,
                            &mut buffer_for_handle,
                            &hangul_repair_for_handle,
                        );
                        Self::schedule_macos_ime_discard(ed);
                    }
                    let clicked_pos = ed.xy_to_position(
                        fltk::app::event_x(),
                        fltk::app::event_y(),
                        PositionType::Cursor,
                    );
                    if clicked_pos >= 0 {
                        Self::remember_preferred_insert_position(
                            &preferred_insert_position_for_handle,
                            &buffer_for_handle,
                            clicked_pos,
                        );
                    }
                    let event_button = fltk::app::event_button();
                    let state = fltk::app::event_state();
                    let ctrl_or_cmd = state.contains(fltk::enums::Shortcut::Ctrl)
                        || state.contains(fltk::enums::Shortcut::Command);

                    if ctrl_or_cmd
                        && (event_button == 1
                            || event_button == fltk::app::MouseButton::Right as i32)
                    {
                        let pos = clicked_pos;
                        if pos >= 0 {
                            let (pos, _) = Self::cursor_position(&buffer_for_handle, pos);
                            if let Some((_, start, end)) = Self::identifier_at_position(
                                &buffer_for_handle,
                                &text_shadow_for_handle,
                                pos,
                            ) {
                                buffer_for_handle.select(start, end);
                                ed.set_insert_position(end);
                            } else {
                                buffer_for_handle.unselect();
                                ed.set_insert_position(pos);
                            }
                            ed.show_insert_position();
                            Self::sync_preferred_insert_position_from_editor(
                                &preferred_insert_position_for_handle,
                                ed,
                                &buffer_for_handle,
                            );
                            widget_for_shortcuts.quick_describe_at_cursor();
                            return true;
                        }
                    }

                    if event_button == fltk::app::MouseButton::Right as i32 {
                        let selected_text = buffer_for_handle.selection_text();
                        let selected_text_is_empty = selected_text.trim().is_empty();
                        let mut clicked_reference_found = false;
                        let mut clicked_reference = None;

                        if clicked_pos >= 0 {
                            let (pos, _) = Self::cursor_position(&buffer_for_handle, clicked_pos);
                            if let Some((reference, start, end)) =
                                Self::object_context_reference_at_position(
                                    &buffer_for_handle,
                                    &text_shadow_for_handle,
                                    pos,
                                )
                            {
                                clicked_reference_found = true;
                                if selected_text_is_empty {
                                    buffer_for_handle.select(start, end);
                                    ed.set_insert_position(end);
                                    ed.show_insert_position();
                                    Self::sync_preferred_insert_position_from_editor(
                                        &preferred_insert_position_for_handle,
                                        ed,
                                        &buffer_for_handle,
                                    );
                                }
                                clicked_reference = Some(reference);
                            }
                        }

                        let candidates = Self::right_click_object_context_candidates(
                            clicked_reference.as_deref(),
                            &selected_text,
                        );
                        if !candidates.is_empty() {
                            let data = intellisense_data_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .clone();
                            for candidate in candidates {
                                if Self::invoke_object_context_callback(
                                    &object_context_callback_for_handle,
                                    candidate,
                                    data.clone(),
                                ) {
                                    return true;
                                }
                            }
                        }

                        if selected_text_is_empty && clicked_pos >= 0 && !clicked_reference_found {
                            let (pos, _) = Self::cursor_position(&buffer_for_handle, clicked_pos);
                            buffer_for_handle.unselect();
                            ed.set_insert_position(pos);
                            ed.show_insert_position();
                            Self::sync_preferred_insert_position_from_editor(
                                &preferred_insert_position_for_handle,
                                ed,
                                &buffer_for_handle,
                            );
                        }
                        Self::show_editor_context_menu(ed, &context_action_callback_for_handle);
                        return true;
                    }
                    false
                }
                Event::KeyDown => {
                    let key = fltk::app::event_key();
                    let original_key = fltk::app::event_original_key();
                    crate::ui::sql_editor::ime_trace(|| {
                        format!(
                            "KeyDown key={key:?} text={:?} compose_state={} caret={} selection={:?}",
                            fltk::app::event_text(),
                            fltk::app::compose_state(),
                            ed.insert_position(),
                            buffer_for_handle.selection_position(),
                        )
                    });
                    #[cfg(target_os = "macos")]
                    if Self::nav_key_is_duplicate_dispatch(&nav_key_dedupe_for_handle, key) {
                        crate::ui::sql_editor::ime_trace(|| {
                            format!("swallow duplicate nav dispatch {key:?}")
                        });
                        return true;
                    }
                    let shortcut_key = Self::shortcut_key_for_layout(key, original_key);
                    let popup_visible = intellisense_popup_for_handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .is_visible();
                    let state = fltk::app::event_state();
                    let ctrl_or_cmd = state.contains(fltk::enums::Shortcut::Ctrl)
                        || state.contains(fltk::enums::Shortcut::Command);
                    let shift = state.contains(fltk::enums::Shortcut::Shift);
                    let alt = state.contains(fltk::enums::Shortcut::Alt);
                    keydown_buffer_revision_for_handle =
                        intellisense_runtime_for_handle.current_buffer_revision();
                    keydown_had_ctrl_or_cmd_for_handle = ctrl_or_cmd;
                    keydown_had_alt_for_handle = alt;

                    if Self::should_hide_intellisense_on_modifier_keydown(popup_visible, key) {
                        intellisense_popup_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .hide();
                        Self::invalidate_and_clear_pending_intellisense_state(
                            &intellisense_runtime_for_handle,
                        );
                        return false;
                    }

                    #[cfg(target_os = "macos")]
                    if Self::handle_macos_user_selection_text_input(
                        ed,
                        &mut buffer_for_handle,
                        &hangul_repair_for_handle,
                        &macos_ime_marked_text_for_handle,
                    ) {
                        return true;
                    }

                    #[cfg(target_os = "macos")]
                    if Self::key_may_change_cursor_or_selection(key, shortcut_key, ctrl_or_cmd) {
                        Self::finish_macos_ime_before_cursor_or_selection_change(
                            ed,
                            &mut buffer_for_handle,
                            &hangul_repair_for_handle,
                        );
                        // Only Cmd/Ctrl-modified keys bypass the IME; plain
                        // arrows are seen and finalized by the IME itself.
                        if ctrl_or_cmd {
                            Self::schedule_macos_ime_discard(ed);
                        }
                    }

                    if ctrl_or_cmd && shift && matches!(key, Key::Up | Key::Down) {
                        if popup_visible {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                        }
                        Self::invalidate_and_clear_pending_intellisense_state(
                            &intellisense_runtime_for_handle,
                        );
                        let direction = if key == Key::Up { -1 } else { 1 };
                        widget_for_shortcuts.select_block_in_direction(direction);
                        return true;
                    }

                    if shortcut_key == Key::Escape {
                        if popup_visible {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                        }
                        return Self::cancel_intellisense_on_escape_keydown(
                            popup_visible,
                            &intellisense_runtime_for_handle,
                        );
                    }

                    if popup_visible {
                        match shortcut_key {
                            Key::Up => {
                                // Navigate popup up, consume event
                                let pos = ed.insert_position();
                                *navigation_keyup_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    NavigationKeyupState::RestoreCursor { anchor: pos };
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .select_prev();
                                ed.set_insert_position(pos);
                                ed.show_insert_position();

                                return true;
                            }
                            Key::Down => {
                                // Navigate popup down, consume event
                                let pos = ed.insert_position();
                                *navigation_keyup_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    NavigationKeyupState::RestoreCursor { anchor: pos };
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .select_next();
                                ed.set_insert_position(pos);
                                ed.show_insert_position();

                                return true;
                            }
                            Key::PageUp => {
                                let pos = ed.insert_position();
                                *navigation_keyup_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    NavigationKeyupState::RestoreCursor { anchor: pos };
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .select_prev_page();
                                ed.set_insert_position(pos);
                                ed.show_insert_position();

                                return true;
                            }
                            Key::PageDown => {
                                let pos = ed.insert_position();
                                *navigation_keyup_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    NavigationKeyupState::RestoreCursor { anchor: pos };
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .select_next_page();
                                ed.set_insert_position(pos);
                                ed.show_insert_position();

                                return true;
                            }
                            Key::Enter | Key::KPEnter | Key::Tab => {
                                // Insert selected suggestion, consume event
                                let selected = intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .get_selected();
                                let has_selected = selected.is_some();
                                if let Some(selected) = selected {
                                    let (cursor_pos, cursor_pos_usize) =
                                        Self::editor_cursor_position(ed, &buffer_for_handle);
                                    let range = intellisense_runtime_for_handle.completion_range();
                                    let (start, end) = Self::completion_replacement_range(
                                        &buffer_for_handle,
                                        &text_shadow_for_handle,
                                        cursor_pos,
                                        range,
                                    );

                                    let inserted = Self::completion_insert_text(&selected);
                                    let completion_changes_text = Self::completion_changes_text(
                                        &buffer_for_handle,
                                        start,
                                        end,
                                        &inserted,
                                    );
                                    {
                                        let mut undo_state = undo_redo_state_for_handle
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                                        if completion_changes_text {
                                            undo_state
                                                .record_cursor_move_to_if_remote(cursor_pos_usize);
                                        }
                                        undo_state.sync_current_cursor(cursor_pos_usize);
                                        if completion_changes_text {
                                            undo_state.prepare_completion_edit();
                                        } else {
                                            undo_state.finish_active_group();
                                        }
                                    }
                                    let caret_offset = Self::completion_caret_offset(&inserted);
                                    if start != end {
                                        buffer_for_handle.replace(
                                            start as i32,
                                            end as i32,
                                            &inserted,
                                        );
                                        ed.set_insert_position((start + caret_offset) as i32);
                                    } else {
                                        buffer_for_handle.insert(cursor_pos, &inserted);
                                        ed.set_insert_position(
                                            (cursor_pos_usize + caret_offset) as i32,
                                        );
                                    }
                                    let after_cursor = if start != end {
                                        start.saturating_add(caret_offset)
                                    } else {
                                        cursor_pos_usize.saturating_add(caret_offset)
                                    };
                                    undo_redo_state_for_handle
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .finish_completion_edit_cursor(
                                            after_cursor,
                                            completion_changes_text,
                                        );
                                    Self::finalize_completion_after_selection(
                                        &intellisense_runtime_for_handle,
                                    );
                                }
                                if matches!(key, Key::Enter | Key::KPEnter) {
                                    *enter_keyup_suppression_for_handle
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                        EnterKeyupSuppression::PopupConfirm;
                                }
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .hide();
                                intellisense_runtime_for_handle.clear_pending_intellisense();
                                if has_selected {
                                    widget_for_shortcuts.schedule_signature_hint_update();
                                }
                                return Self::should_consume_popup_confirm_key(key, has_selected);
                            }
                            _ => {
                                // Let other keys pass through to editor
                            }
                        }
                    }

                    if !ed.active() || (!ed.has_focus() && !popup_visible) {
                        return false;
                    }
                    // KeyDown fires BEFORE the character is inserted into the buffer.
                    // Handle navigation and selection keys here to consume them
                    // before they affect the editor.

                    // Handle basic editing shortcuts
                    let ctrl_or_cmd = state.contains(fltk::enums::Shortcut::Ctrl)
                        || state.contains(fltk::enums::Shortcut::Command);
                    let shift = state.contains(fltk::enums::Shortcut::Shift);

                    if ctrl_or_cmd {
                        if shift && Self::matches_alpha_shortcut(shortcut_key, 'f') {
                            widget_for_shortcuts.format_selected_sql();
                            return true;
                        }

                        match shortcut_key {
                            k if Self::matches_alpha_shortcut(k, 'z') => {
                                widget_for_shortcuts.undo();
                                return true;
                            }
                            k if Self::matches_alpha_shortcut(k, 'y') => {
                                widget_for_shortcuts.redo();
                                return true;
                            }
                            k if k == Key::from_char(' ') => {
                                // Ctrl+Space - Trigger intellisense
                                Self::invalidate_manual_trigger_debounce_state(
                                    &intellisense_runtime_for_handle,
                                );
                                Self::trigger_intellisense(
                                    ed,
                                    &buffer_for_handle,
                                    &text_shadow_for_handle,
                                    &intellisense_data_for_handle,
                                    &intellisense_popup_for_handle,
                                    &column_sender_for_handle,
                                    &connection_for_handle,
                                    &intellisense_runtime_for_handle,
                                );
                                return true;
                            }
                            Key::Enter | Key::KPEnter => {
                                let should_suppress = {
                                    let mut suppression = enter_keyup_suppression_for_handle
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    Self::should_suppress_ctrl_enter_dispatch(
                                        &mut suppression,
                                        std::time::Instant::now(),
                                    )
                                };
                                if should_suppress {
                                    return true;
                                }
                                widget_for_shortcuts.execute_statement_at_cursor();
                                return true;
                            }
                            k if Self::matches_alpha_shortcut(k, 'f') => {
                                Self::invoke_void_callback(&find_callback_for_handle);
                                return true;
                            }
                            k if k == Key::from_char('/') || k == Key::from_char('?') => {
                                widget_for_shortcuts.toggle_comment();
                                return true;
                            }
                            k if Self::matches_alpha_shortcut(k, 'u') => {
                                widget_for_shortcuts.convert_selection_case(true);
                                return true;
                            }
                            k if Self::matches_alpha_shortcut(k, 'l') => {
                                widget_for_shortcuts.convert_selection_case(false);
                                return true;
                            }
                            k if Self::matches_alpha_shortcut(k, 'h') => {
                                Self::invoke_void_callback(&replace_callback_for_handle);
                                return true;
                            }
                            _ => {}
                        }
                    }

                    #[cfg(target_os = "macos")]
                    if !alt
                        && matches!(key, Key::Enter | Key::KPEnter)
                        && matches!(
                            *enter_keyup_suppression_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            EnterKeyupSuppression::ImeCompositionEnter
                        )
                    {
                        return true;
                    }

                    #[cfg(target_os = "macos")]
                    if !alt
                        && matches!(key, Key::Enter | Key::KPEnter)
                        && Self::should_handle_enter_during_ime_composition(
                            fltk::app::compose_state(),
                        )
                    {
                        *enter_keyup_suppression_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            EnterKeyupSuppression::ImeCompositionEnter;
                        let handled = Self::handle_ime_enter_auto_indent(
                            ed,
                            &mut buffer_for_handle,
                            &text_shadow_for_handle,
                        );
                        if handled {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                            Self::invalidate_keyup_debounce_with_parse_generation(
                                &intellisense_runtime_for_handle,
                                true,
                            );
                        }
                        return true;
                    }

                    if !alt && matches!(key, Key::Enter | Key::KPEnter) {
                        let handled = Self::handle_enter_auto_indent(
                            ed,
                            &mut buffer_for_handle,
                            &text_shadow_for_handle,
                        );
                        if handled {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                            Self::invalidate_keyup_debounce_with_parse_generation(
                                &intellisense_runtime_for_handle,
                                true,
                            );
                            return true;
                        }
                    }

                    // F4 - Quick Describe (handle on KeyDown for immediate response)
                    if key == Key::F4 {
                        widget_for_shortcuts.quick_describe_at_cursor();
                        return true;
                    }

                    if key == Key::F3 {
                        let mut editor_for_find = ed.clone();
                        if !FindReplaceDialog::find_next_from_session(
                            &mut editor_for_find,
                            &mut buffer_for_handle,
                        ) && !FindReplaceDialog::has_search_text()
                        {
                            Self::invoke_void_callback(&find_callback_for_handle);
                        }
                        return true;
                    }

                    if key == Key::F5 {
                        widget_for_shortcuts.execute_current();
                        return true;
                    }

                    if key == Key::F9 {
                        widget_for_shortcuts.execute_statement_at_cursor();
                        return true;
                    }

                    if key == Key::F6 {
                        widget_for_shortcuts.explain_current();
                        return true;
                    }

                    if key == Key::F7 {
                        widget_for_shortcuts.commit();
                        return true;
                    }

                    if key == Key::F8 {
                        widget_for_shortcuts.rollback();
                        return true;
                    }

                    false
                }
                Event::KeyUp => {
                    crate::ui::sql_editor::ime_trace(|| {
                        format!(
                            "KeyUp key={:?} text={:?} compose_state={} caret={} selection={:?}",
                            fltk::app::event_key(),
                            fltk::app::event_text(),
                            fltk::app::compose_state(),
                            ed.insert_position(),
                            buffer_for_handle.selection_position(),
                        )
                    });
                    // The Hangul first-key repair is evaluated on KeyUp:
                    // unlike KeyDown, a KeyUp always arrives after the
                    // keystroke's text/compose state has been applied,
                    // regardless of whether the custom handler runs before or
                    // after the editor's default handling.
                    #[cfg(target_os = "macos")]
                    {
                        Self::remember_macos_ime_marked_text(
                            ed,
                            &buffer_for_handle,
                            &macos_ime_marked_text_for_handle,
                        );
                        let mods = fltk::app::event_state();
                        let has_command_modifiers = mods
                            .contains(fltk::enums::Shortcut::Ctrl)
                            || mods.contains(fltk::enums::Shortcut::Command)
                            || mods.contains(fltk::enums::Shortcut::Alt);
                        let reader = buffer_for_handle.clone();
                        let edit = hangul_repair_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .on_key_event(
                                &fltk::app::event_text(),
                                has_command_modifiers,
                                fltk::app::compose_state().max(0) as usize,
                                ed.insert_position().max(0) as usize,
                                &|start, end| reader.text_range(start as i32, end as i32),
                            );
                        Self::apply_hangul_first_key_repair_edit(&mut buffer_for_handle, edit);
                    }
                    let popup_visible = intellisense_popup_for_handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .is_visible();
                    if !ed.active() || (!ed.has_focus() && !popup_visible) {
                        return false;
                    }
                    Self::sync_preferred_insert_position_from_editor(
                        &preferred_insert_position_for_handle,
                        ed,
                        &buffer_for_handle,
                    );
                    // KeyUp fires AFTER the character is inserted into the buffer.
                    // Filter/show intellisense here.
                    let key = fltk::app::event_key();
                    let original_key = fltk::app::event_original_key();
                    let event_text = fltk::app::event_text();
                    let state = fltk::app::event_state();
                    let ctrl_or_cmd = state.contains(fltk::enums::Shortcut::Ctrl)
                        || state.contains(fltk::enums::Shortcut::Command);
                    let alt = state.contains(fltk::enums::Shortcut::Alt);
                    let shift = state.contains(fltk::enums::Shortcut::Shift);
                    let keyup_ctrl_or_cmd = ctrl_or_cmd || keydown_had_ctrl_or_cmd_for_handle;
                    let shortcut_modified =
                        keyup_ctrl_or_cmd || alt || keydown_had_alt_for_handle;
                    let buffer_changed_since_keydown = intellisense_runtime_for_handle
                        .current_buffer_revision()
                        != keydown_buffer_revision_for_handle;
                    let should_process_text_input = Self::should_process_keyup_text_input(
                        buffer_changed_since_keydown,
                        shortcut_modified,
                    );
                    if matches!(
                        key,
                        Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown
                    ) {
                        widget_for_shortcuts.schedule_deferred_visible_semantic_rehighlight();
                    }

                    // Ctrl/Cmd+Space is handled on KeyDown for manual intellisense trigger.
                    // Ignore the matching KeyUp so the popup is not immediately dismissed.
                    if Self::should_ignore_keyup_after_manual_trigger(
                        key,
                        original_key,
                        keyup_ctrl_or_cmd,
                    ) {
                        return true;
                    }

                    // Keep KeyUp lightweight by using raw offsets (no full-buffer clones).
                    let cursor_pos = ed.insert_position();
                    let char_before_cursor = Self::char_before_cursor(
                        &buffer_for_handle,
                        &text_shadow_for_handle,
                        cursor_pos,
                    );
                    let typed_char = Self::typed_char_from_key_event(
                        &event_text,
                        key,
                        shift,
                        char_before_cursor,
                    );
                    if Self::is_modifier_key(key) {
                        return false;
                    }

                    let consume_suppressed_enter_keyup = {
                        let mut suppression = enter_keyup_suppression_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        Self::take_enter_keyup_suppression(key, &mut suppression)
                    };
                    if consume_suppressed_enter_keyup {
                        return true;
                    }

                    if shortcut_modified {
                        if buffer_changed_since_keydown
                            || Self::should_refresh_signature_hint_after_keyup(false, key)
                        {
                            widget_for_shortcuts.schedule_signature_hint_update();
                        }
                        if popup_visible {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                        }
                        Self::invalidate_and_clear_pending_intellisense_state(
                            &intellisense_runtime_for_handle,
                        );
                        return false;
                    }

                    // Re-evaluate the signature on any buffer change or caret
                    // movement. The parse is bounded and coalesced, so running
                    // it every keystroke keeps the popup self-healing: a missed
                    // '(' event recovers on the next key instead of leaving the
                    // hint permanently off.
                    if Self::should_refresh_signature_hint_after_keyup(
                        buffer_changed_since_keydown,
                        key,
                    ) {
                        widget_for_shortcuts.schedule_signature_hint_update();
                    }

                    if event_text.is_empty()
                        && typed_char.is_none()
                        && !ctrl_or_cmd
                        && !alt
                        && !matches!(
                            key,
                            Key::BackSpace
                                | Key::Delete
                                | Key::Left
                                | Key::Right
                                | Key::Up
                                | Key::Down
                                | Key::Home
                                | Key::End
                                | Key::PageUp
                                | Key::PageDown
                                | Key::Enter
                                | Key::KPEnter
                                | Key::Tab
                                | Key::Escape
                        )
                    {
                        if popup_visible {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                            Self::invalidate_keyup_debounce_with_parse_generation(
                                &intellisense_runtime_for_handle,
                                true,
                            );
                        }
                        return false;
                    }

                    if matches!(key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
                        let mut nav_state = navigation_keyup_state_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if let NavigationKeyupState::RestoreCursor { anchor } = *nav_state {
                            ed.set_insert_position(anchor);
                            ed.show_insert_position();
                            Self::sync_preferred_insert_position_from_editor(
                                &preferred_insert_position_for_handle,
                                ed,
                                &buffer_for_handle,
                            );
                            *nav_state = NavigationKeyupState::Idle;
                            return true;
                        }
                    }

                    // Navigation keys - hide popup and let editor handle cursor movement
                    if matches!(
                        key,
                        Key::Left | Key::Right | Key::Home | Key::End | Key::PageUp | Key::PageDown
                    ) {
                        if popup_visible {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                        }
                        Self::invalidate_keyup_debounce_with_parse_generation(
                            &intellisense_runtime_for_handle,
                            true,
                        );
                        return false;
                    }

                    // Skip if these keys (already handled in KeyDown)
                    if popup_visible
                        && matches!(
                            key,
                            Key::Up
                                | Key::Down
                                | Key::PageUp
                                | Key::PageDown
                                | Key::Escape
                                | Key::Enter
                                | Key::KPEnter
                                | Key::Tab
                        )
                    {
                        return true;
                    }

                    // Handle typing - update intellisense filter
                    let preferred_db_type = intellisense_runtime_for_handle
                        .db_type_without_blocking(&connection_for_handle);
                    let buffer_len = buffer_for_handle.length();

                    let fast_path_applied = if popup_visible {
                        Self::try_fast_path_intellisense_filter(
                            ed,
                            &buffer_for_handle,
                            &text_shadow_for_handle,
                            &intellisense_popup_for_handle,
                            &intellisense_runtime_for_handle,
                            Some(preferred_db_type),
                            cursor_pos,
                            key,
                            typed_char,
                        )
                    } else {
                        false
                    };
                    let (word, word_start, _) = if fast_path_applied {
                        (String::new(), cursor_pos.max(0) as usize, cursor_pos.max(0) as usize)
                    } else {
                        Self::word_at_cursor_for_db(
                            &buffer_for_handle,
                            &text_shadow_for_handle,
                            cursor_pos,
                            Some(preferred_db_type),
                        )
                    };

                    if fast_path_applied {
                        // The fast path only filters the open popup; it never
                        // re-runs analysis. If a column load is still in flight
                        // (e.g. the other-table columns backing a qualified `=`
                        // comparison suggestion), keep its pending refresh alive
                        // and re-point it at the new caret so the load-completion
                        // refresh still matches. Clearing it here permanently
                        // dropped those late suggestions until the user deleted
                        // and retyped.
                        let (normalized_cursor, _) =
                            Self::editor_cursor_position(ed, &buffer_for_handle);
                        intellisense_runtime_for_handle
                            .retarget_pending_intellisense(normalized_cursor);
                        Self::invalidate_keyup_debounce_with_parse_generation(
                            &intellisense_runtime_for_handle,
                            true,
                        );
                    } else if key == Key::BackSpace || key == Key::Delete {
                        // After backspace/delete, re-evaluate (debounced)
                        if Self::should_auto_trigger_after_delete(&word) {
                            Self::schedule_keyup_intellisense_debounce(
                                &intellisense_runtime_for_handle,
                                cursor_pos,
                                buffer_len,
                                ed,
                                &buffer_for_handle,
                                &text_shadow_for_handle,
                                &intellisense_data_for_handle,
                                &intellisense_popup_for_handle,
                                &column_sender_for_handle,
                                &connection_for_handle,
                            );
                        } else {
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                            Self::invalidate_keyup_debounce_with_parse_generation(
                                &intellisense_runtime_for_handle,
                                true,
                            );
                        }
                    } else if should_process_text_input {
                        let Some(ch) = typed_char else {
                            return false;
                        };
                        if Self::should_force_full_analysis(ch) {
                            let qualifier = Self::qualifier_before_word(
                                &buffer_for_handle,
                                &text_shadow_for_handle,
                                word_start,
                                Some(preferred_db_type),
                            );
                            // A space right after `END` auto-opens the popup when
                            // the slot has a known completion (the enclosing
                            // object's name / the construct qualifier) — those
                            // never get a 2-char prefix to trigger on otherwise.
                            let auto_trigger_base = ch.is_whitespace()
                                && word.is_empty()
                                && qualifier.is_none();
                            let db_type =
                                Some(intellisense_runtime_for_handle.cached_db_type());
                            let cursor = cursor_pos.max(0) as usize;
                            let (context_lookbehind, context_lookahead) =
                                Self::intellisense_context_lookaround(
                                    intellisense_runtime_for_handle.context_window_bytes(),
                                );
                            let end_slot_auto_trigger = if auto_trigger_base {
                                let follows_end = {
                                    let shadow = text_shadow_for_handle
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    let (suffix, _, relative_cursor) =
                                        shadow.bounded_text_around(cursor, 64, 0);
                                    Self::cursor_follows_end_keyword_word_in_text(
                                        &suffix,
                                        relative_cursor,
                                    )
                                };
                                follows_end
                                    && {
                                        let shadow = text_shadow_for_handle
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                                        let (text, _, relative_cursor) = shadow.bounded_text_around(
                                            cursor,
                                            context_lookbehind,
                                            context_lookahead,
                                        );
                                        Self::plsql_end_auto_trigger_applies_in_text(
                                            &text,
                                            relative_cursor,
                                            db_type,
                                        )
                                    }
                            } else {
                                false
                            };
                            let execute_immediate_tail_auto_trigger = if auto_trigger_base {
                                let shadow = text_shadow_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let (text, _, relative_cursor) = shadow.bounded_text_around(
                                    cursor,
                                    context_lookbehind,
                                    0,
                                );
                                Self::plsql_execute_immediate_tail_auto_trigger_applies_in_text(
                                    &text,
                                    relative_cursor,
                                    db_type,
                                )
                            } else {
                                false
                            };
                            if end_slot_auto_trigger
                                || execute_immediate_tail_auto_trigger
                                || Self::should_auto_trigger_intellisense_for_forced_char(
                                    &word,
                                    qualifier.as_deref(),
                                )
                            {
                                Self::schedule_keyup_intellisense_debounce(
                                    &intellisense_runtime_for_handle,
                                    cursor_pos,
                                    buffer_len,
                                    ed,
                                    &buffer_for_handle,
                                    &text_shadow_for_handle,
                                    &intellisense_data_for_handle,
                                    &intellisense_popup_for_handle,
                                    &column_sender_for_handle,
                                    &connection_for_handle,
                                );
                            } else {
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .hide();
                                intellisense_runtime_for_handle.clear_ui_tracking();
                                Self::invalidate_keyup_debounce_with_parse_generation(
                                    &intellisense_runtime_for_handle,
                                    true,
                                );
                            }
                        } else if sql_text::is_identifier_char(ch) {
                            // Alphanumeric typed - show/update popup if word is long enough
                            let qualifier = Self::qualifier_before_word(
                                &buffer_for_handle,
                                &text_shadow_for_handle,
                                word_start,
                                Some(preferred_db_type),
                            );
                            if Self::should_auto_trigger_intellisense_for_identifier_char(
                                &word,
                                qualifier.as_deref(),
                            ) {
                                Self::schedule_keyup_intellisense_debounce(
                                    &intellisense_runtime_for_handle,
                                    cursor_pos,
                                    buffer_len,
                                    ed,
                                    &buffer_for_handle,
                                    &text_shadow_for_handle,
                                    &intellisense_data_for_handle,
                                    &intellisense_popup_for_handle,
                                    &column_sender_for_handle,
                                    &connection_for_handle,
                                );
                            } else {
                                intellisense_popup_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .hide();
                                intellisense_runtime_for_handle.clear_ui_tracking();
                                Self::invalidate_keyup_debounce_with_parse_generation(
                                    &intellisense_runtime_for_handle,
                                    true,
                                );
                            }
                        } else {
                            // Non-identifier character (space, punctuation, etc.)
                            // Close popup - user is done with this word
                            intellisense_popup_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            intellisense_runtime_for_handle.clear_ui_tracking();
                            Self::invalidate_keyup_debounce_with_parse_generation(
                                &intellisense_runtime_for_handle,
                                true,
                            );
                        }
                    }

                    if Self::has_min_intellisense_prefix(&word) {
                        Self::maybe_prefetch_columns_for_word(
                            &word,
                            &intellisense_data_for_handle,
                            &column_sender_for_handle,
                            &connection_for_handle,
                        );
                    }
                    false
                }
                Event::Unfocus => {
                    #[cfg(target_os = "macos")]
                    {
                        fltk::draw::reset_spot();
                        let reader = buffer_for_handle.clone();
                        let edit = hangul_repair_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .flush(reader.length().max(0) as usize, &|start, end| {
                                reader.text_range(start as i32, end as i32)
                            });
                        Self::apply_hangul_first_key_repair_edit(&mut buffer_for_handle, edit);
                    }
                    widget_for_shortcuts
                        .schedule_deferred_signature_unfocus_hide(INTELLISENSE_DEFERRED_HIDE_RETRIES);
                    let unfocus_x = fltk::app::event_x_root();
                    let unfocus_y = fltk::app::event_y_root();
                    if matches!(
                        intellisense_runtime_for_handle.popup_transition_state(),
                        IntellisensePopupTransitionState::Showing
                    ) {
                        Self::schedule_deferred_unfocus_popup_hide(
                            ed.clone(),
                            intellisense_popup_for_handle.clone(),
                            intellisense_runtime_for_handle.clone(),
                            unfocus_x,
                            unfocus_y,
                            INTELLISENSE_DEFERRED_HIDE_RETRIES,
                        );
                        return false;
                    }
                    let should_hide_and_clear = {
                        let mut popup = intellisense_popup_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let popup_visible = popup.is_visible();
                        let pointer_inside_popup =
                            popup_visible && popup.contains_point(unfocus_x, unfocus_y);
                        if Self::should_hide_popup_on_unfocus(popup_visible, pointer_inside_popup) {
                            popup.hide();
                            true
                        } else {
                            false
                        }
                    };
                    if should_hide_and_clear {
                        Self::clear_intellisense_state_for_external_hide(
                            &intellisense_runtime_for_handle,
                        );
                    }
                    false
                }
                Event::Shortcut => {
                    let key = fltk::app::event_key();
                    let popup_visible = intellisense_popup_for_handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .is_visible();
                    let state = fltk::app::event_state();
                    let ctrl_or_cmd = state.contains(fltk::enums::Shortcut::Ctrl)
                        || state.contains(fltk::enums::Shortcut::Command);

                    // If intellisense is visible, consume Enter/Tab to prevent them from reaching other handlers
                    if popup_visible
                        && matches!(
                            key,
                            Key::Up
                                | Key::Down
                                | Key::PageUp
                                | Key::PageDown
                                | Key::Enter
                                | Key::KPEnter
                                | Key::Tab
                        )
                    {
                        return true;
                    }

                    if ctrl_or_cmd && matches!(key, Key::Enter | Key::KPEnter) {
                        let should_suppress = {
                            let mut suppression = enter_keyup_suppression_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            Self::should_suppress_ctrl_enter_dispatch(
                                &mut suppression,
                                std::time::Instant::now(),
                            )
                        };
                        if should_suppress {
                            return true;
                        }
                        widget_for_shortcuts.execute_statement_at_cursor();
                        return true;
                    }

                    false
                }
                Event::Paste => {
                    widget_for_shortcuts.schedule_signature_hint_update();
                    let Some(drop) = Self::take_pending_dnd_drop(&dnd_drop_state_for_handle) else {
                        let event_text = app::event_text();
                        if !event_text.is_empty() {
                            *pending_paste_text_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(Arc::new(event_text));
                        }
                        return false;
                    };
                    pending_paste_text_for_handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    let scroll_origin = Self::take_dnd_scroll_origin(&dnd_scroll_origin_for_handle);
                    Self::set_dnd_drop_target_active(ed, false);

                    let event_text = app::event_text();
                    if let Some(insert_text) = crate::ui::object_drag_payload::decode(&event_text)
                        .or_else(|| {
                            crate::ui::object_drag_payload::take_active_drag_text(&event_text)
                        })
                    {
                        Self::insert_text_at_dnd_drop_position(
                            ed,
                            &mut buffer_for_handle,
                            &preferred_insert_position_for_handle,
                            drop.position,
                            &insert_text,
                            scroll_origin,
                        );
                        return true;
                    }

                    Self::restore_editor_scroll(ed, scroll_origin);
                    if let Some(path) = Self::extract_dropped_file_path(&event_text) {
                        if Self::invoke_file_drop_callback(&file_drop_callback_for_handle, path) {
                            return true;
                        }
                    }
                    false
                }
                _ => false,
            }
        });
    }

    fn set_dnd_drop_state(state: &Arc<Mutex<DndDropState>>, next: DndDropState) {
        *state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    fn dnd_scroll_origin(
        slot: &Arc<Mutex<Option<(i32, i32)>>>,
        editor: &TextEditor,
    ) -> (i32, i32) {
        let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard.get_or_insert_with(|| (editor.scroll_row(), editor.scroll_col()))
    }

    fn set_dnd_scroll_origin(
        slot: &Arc<Mutex<Option<(i32, i32)>>>,
        editor: &TextEditor,
    ) -> (i32, i32) {
        let scroll_position = (editor.scroll_row(), editor.scroll_col());
        *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(scroll_position);
        scroll_position
    }

    fn take_dnd_scroll_origin(slot: &Arc<Mutex<Option<(i32, i32)>>>) -> Option<(i32, i32)> {
        slot.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn restore_editor_scroll(editor: &mut TextEditor, scroll_position: Option<(i32, i32)>) {
        if let Some((row, col)) = scroll_position {
            editor.scroll(row, col);
        }
    }

    fn set_dnd_drop_target_active(editor: &mut TextEditor, active: bool) {
        editor.set_color(if active {
            theme::input_bg()
        } else {
            theme::editor_bg()
        });
        editor.redraw();
    }

    fn move_editor_cursor_to_dnd_drop_position(
        editor: &mut TextEditor,
        buffer: &TextBuffer,
        preferred_insert_position: &Arc<Mutex<Option<i32>>>,
        scroll_position: Option<(i32, i32)>,
    ) -> Option<i32> {
        let pos = editor.xy_to_position(
            fltk::app::event_x(),
            fltk::app::event_y(),
            PositionType::Cursor,
        );
        if pos < 0 {
            return None;
        }

        let (pos, _) = Self::cursor_position(buffer, pos);
        let _ = editor.take_focus();
        editor.set_insert_position(pos);
        Self::restore_editor_scroll(editor, scroll_position);
        editor.redraw();
        Self::remember_preferred_insert_position(preferred_insert_position, buffer, pos);
        Some(pos)
    }

    fn should_skip_pointer_position_tracking(state: &Arc<Mutex<DndDropState>>) -> bool {
        matches!(
            *state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            DndDropState::AwaitingPaste(_)
        )
    }

    fn take_pending_dnd_drop(state: &Arc<Mutex<DndDropState>>) -> Option<PendingDndDrop> {
        let mut drop_state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending = match *drop_state {
            DndDropState::Idle => None,
            DndDropState::AwaitingPaste(drop) => Some(drop),
        };
        *drop_state = DndDropState::Idle;
        pending
    }

    fn insert_text_at_dnd_drop_position(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        preferred_insert_position: &Arc<Mutex<Option<i32>>>,
        drop_position: Option<i32>,
        text: &str,
        scroll_position: Option<(i32, i32)>,
    ) {
        let fallback = load_mutex_i32_option(preferred_insert_position)
            .unwrap_or_else(|| editor.insert_position());
        let insert_pos = drop_position.unwrap_or(fallback);
        let (insert_pos, insert_pos_usize) = Self::cursor_position(buffer, insert_pos);
        buffer.insert(insert_pos, text);
        let new_pos = insert_pos_usize.saturating_add(text.len());
        let _ = editor.take_focus();
        editor.set_insert_position(new_pos as i32);
        Self::restore_editor_scroll(editor, scroll_position);
        editor.redraw();
        Self::remember_preferred_insert_position(preferred_insert_position, buffer, new_pos as i32);
    }

    fn extract_dropped_file_path(raw: &str) -> Option<PathBuf> {
        for token in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if token.starts_with('#') {
                continue;
            }
            let Some(path) = Self::parse_dropped_file_token(token) else {
                continue;
            };
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn parse_dropped_file_token(token: &str) -> Option<PathBuf> {
        let cleaned = token.trim_matches('\0').trim();
        let cleaned = cleaned
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                cleaned
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(cleaned)
            .trim();
        if cleaned.is_empty() {
            return None;
        }

        let path_str = if let Some(rest) = Self::strip_prefix_ignore_ascii_case(cleaned, "file://")
        {
            let mut uri_path = rest.trim();
            if let Some(after_localhost) =
                Self::strip_prefix_ignore_ascii_case(uri_path, "localhost")
            {
                uri_path = after_localhost;
            }
            #[cfg(windows)]
            {
                let bytes = uri_path.as_bytes();
                if bytes.len() >= 3
                    && bytes[0] == b'/'
                    && bytes[1].is_ascii_alphabetic()
                    && bytes[2] == b':'
                {
                    uri_path = &uri_path[1..];
                }
            }
            Self::decode_uri_percent(uri_path)
        } else {
            cleaned.to_string()
        };

        if path_str.is_empty() {
            return None;
        }
        Some(PathBuf::from(path_str))
    }

    fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
        let value_bytes = value.as_bytes();
        let prefix_bytes = prefix.as_bytes();
        if value_bytes.len() < prefix_bytes.len() {
            return None;
        }
        if value_bytes[..prefix_bytes.len()].eq_ignore_ascii_case(prefix_bytes) {
            return value.get(prefix_bytes.len()..);
        }
        None
    }

    fn decode_uri_percent(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex_value = |b: u8| -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
                {
                    out.push((high << 4) | low);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(out)
            .unwrap_or_else(|err| String::from_utf8_lossy(&err.into_bytes()).into_owned())
    }
}
