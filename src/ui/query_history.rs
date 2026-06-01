use fltk::{
    app,
    browser::HoldBrowser,
    button::{Button, CheckButton},
    enums::{Event, FrameType, Key, Shortcut},
    group::Flex,
    input::Input,
    prelude::*,
    text::{StyleTableEntry, TextBuffer, TextDisplay},
    window::Window,
};
use std::sync::{mpsc, OnceLock};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::syntax_highlight::{encode_fltk_style_bytes, set_text_buffer_raw_bytes};
use crate::ui::theme;
use crate::ui::{configured_editor_profile, configured_ui_font_size};
use crate::utils::config::{QueryHistory, QueryHistoryEntry};

enum HistoryCommand {
    Add(PendingHistoryEntry),
    Clear(mpsc::Sender<Result<(), String>>),
    Snapshot(mpsc::Sender<Vec<QueryHistoryEntry>>),
}

#[derive(Debug, Clone)]
struct PendingHistoryEntry {
    sql: String,
    timestamp: String,
    execution_time_ms: u64,
    row_count: usize,
    connection_name: String,
    success: bool,
    message: String,
}

const HISTORY_WRITER_RESPONSE_TIMEOUT_DEFAULT_SECS: u64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryTextShortcutAction {
    SelectAll,
    Copy,
}

fn fold_for_case_insensitive(value: &str) -> String {
    value.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

fn history_writer_response_timeout() -> std::time::Duration {
    std::env::var("SPACE_QUERY_HISTORY_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(|| {
            std::time::Duration::from_secs(HISTORY_WRITER_RESPONSE_TIMEOUT_DEFAULT_SECS)
        })
}

fn materialize_history_entry(entry: PendingHistoryEntry) -> QueryHistoryEntry {
    let error_message = if entry.success {
        None
    } else {
        Some(entry.message)
    };
    let error_line = error_message.as_deref().and_then(parse_error_line);

    QueryHistoryEntry {
        sql: entry.sql,
        timestamp: entry.timestamp,
        execution_time_ms: entry.execution_time_ms,
        row_count: entry.row_count,
        connection_name: entry.connection_name,
        success: entry.success,
        error_message,
        error_line,
    }
}

fn spawn_history_writer() -> mpsc::Sender<HistoryCommand> {
    let (sender, receiver) = mpsc::channel::<HistoryCommand>();
    thread::spawn(move || {
        let mut history = QueryHistory::load();

        loop {
            let cmd = match receiver.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            };

            let mut snapshot_replies: Vec<mpsc::Sender<Vec<QueryHistoryEntry>>> = Vec::new();
            let mut clear_replies: Vec<mpsc::Sender<Result<(), String>>> = Vec::new();

            let mut apply_command = |command: HistoryCommand| match command {
                HistoryCommand::Add(entry) => {
                    history.add_entry(materialize_history_entry(entry));
                }
                HistoryCommand::Clear(reply) => {
                    history.queries.clear();
                    clear_replies.push(reply);
                }
                HistoryCommand::Snapshot(reply) => {
                    snapshot_replies.push(reply);
                }
            };

            apply_command(cmd);
            while let Ok(next) = receiver.try_recv() {
                apply_command(next);
            }

            let snapshot: Vec<QueryHistoryEntry> = history.queries.iter().cloned().collect();
            for reply in snapshot_replies {
                let _ = reply.send(snapshot.clone());
            }
            for reply in clear_replies {
                let _ = reply.send(Ok(()));
            }
        }
    });
    sender
}

fn history_writer_handle() -> &'static Mutex<mpsc::Sender<HistoryCommand>> {
    static HISTORY_WRITER: OnceLock<Mutex<mpsc::Sender<HistoryCommand>>> = OnceLock::new();
    HISTORY_WRITER.get_or_init(|| Mutex::new(spawn_history_writer()))
}

fn history_writer_sender() -> mpsc::Sender<HistoryCommand> {
    history_writer_handle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn send_history_command(command: HistoryCommand) -> Result<(), mpsc::SendError<HistoryCommand>> {
    let initial_sender = history_writer_sender();
    let command = match initial_sender.send(command) {
        Ok(()) => return Ok(()),
        Err(err) => err.0,
    };

    let mut guard = history_writer_handle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = spawn_history_writer();
    guard.send(command)
}

fn parse_error_line(message: &str) -> Option<usize> {
    let lowercase = fold_for_case_insensitive(message);
    let patterns = [
        "error at line",
        "near line",
        "line:",
        " at line ",
        // Keep ORA-06512 lower priority so we prefer primary parser errors.
        "ora-06512: at line",
    ];

    let mut best_line: Option<(usize, usize)> = None;

    for (priority, needle) in patterns.iter().enumerate() {
        let mut search_start = 0usize;
        while search_start < lowercase.len() {
            let Some(relative_idx) = lowercase[search_start..].find(needle) else {
                break;
            };
            let idx = search_start + relative_idx;
            let mut cursor = idx + needle.len();
            cursor = next_char_boundary(&lowercase, cursor);

            let mut digits = String::new();
            while cursor < lowercase.len() {
                let Some((ch, next)) = next_char_with_clamped_boundary(&lowercase, cursor) else {
                    break;
                };
                if ch.is_whitespace() && digits.is_empty() {
                    cursor = next;
                    continue;
                }
                if ch.is_ascii_digit() {
                    digits.push(ch);
                    cursor = next;
                    continue;
                }
                break;
            }

            if let Ok(value) = digits.parse::<usize>() {
                match best_line {
                    None => best_line = Some((priority, value)),
                    Some((best_priority, _)) if priority < best_priority => {
                        best_line = Some((priority, value));
                    }
                    _ => {}
                }
            }
            search_start = idx.saturating_add(needle.len());
        }
    }

    best_line.map(|(_, line)| line).filter(|line| *line > 0)
}

fn next_char_boundary(text: &str, idx: usize) -> usize {
    let mut boundary = idx.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn next_char_with_clamped_boundary(text: &str, idx: usize) -> Option<(char, usize)> {
    let start = next_char_boundary(text, idx);
    if start >= text.len() {
        return None;
    }

    let ch = text[start..].chars().next()?;
    Some((ch, start + ch.len_utf8()))
}

fn build_preview_styles(sql: &str, error_line: Option<usize>) -> String {
    if sql.is_empty() {
        return String::new();
    }
    let mut styles = String::with_capacity(sql.len());
    let mut line_number = 1usize;
    for line in sql.split_inclusive('\n') {
        let style_char = if error_line == Some(line_number) {
            'B'
        } else {
            'A'
        };
        styles.extend(std::iter::repeat_n(style_char, line.len()));
        line_number = line_number.saturating_add(1);
    }
    styles
}

fn set_preview_style_buffer(
    style_buffer: &mut TextBuffer,
    sql: &str,
    logical_styles: &str,
) -> bool {
    let Some(encoded) = encode_fltk_style_bytes(sql, logical_styles) else {
        return false;
    };
    set_text_buffer_raw_bytes(style_buffer, &encoded);
    true
}

fn matches_history_shortcut_key(key: Key, original_key: Key, ascii: char) -> bool {
    let lower = Key::from_char(ascii.to_ascii_lowercase());
    let upper = Key::from_char(ascii.to_ascii_uppercase());
    key == lower || key == upper || original_key == lower || original_key == upper
}

fn resolve_history_text_shortcut_action(
    key: Key,
    original_key: Key,
    state: Shortcut,
) -> Option<HistoryTextShortcutAction> {
    let ctrl_or_cmd = state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Command);
    if !ctrl_or_cmd {
        return None;
    }

    if matches_history_shortcut_key(key, original_key, 'a') {
        return Some(HistoryTextShortcutAction::SelectAll);
    }
    if matches_history_shortcut_key(key, original_key, 'c') {
        return Some(HistoryTextShortcutAction::Copy);
    }

    None
}

fn install_history_text_shortcuts(display: &mut TextDisplay, mut buffer: TextBuffer) {
    display.handle(move |widget, ev| match ev {
        Event::KeyDown | Event::Shortcut => {
            let key = app::event_key();
            let original_key = app::event_original_key();
            let state = app::event_state();

            match resolve_history_text_shortcut_action(key, original_key, state) {
                Some(HistoryTextShortcutAction::SelectAll) => {
                    let end = buffer.length().max(0);
                    buffer.select(0, end);
                    widget.redraw();
                    true
                }
                Some(HistoryTextShortcutAction::Copy) => {
                    let selected = buffer.selection_text();
                    if selected.is_empty() {
                        false
                    } else {
                        app::copy(&selected);
                        true
                    }
                }
                None => false,
            }
        }
        _ => false,
    });
}

fn preview_style_table() -> Vec<StyleTableEntry> {
    let profile = configured_editor_profile();
    let size = configured_ui_font_size();
    vec![
        StyleTableEntry {
            color: theme::text_primary(),
            font: profile.normal,
            size,
        },
        StyleTableEntry {
            color: theme::button_danger(),
            font: profile.normal,
            size,
        },
    ]
}

fn load_snapshot() -> Vec<QueryHistoryEntry> {
    let (tx, rx) = mpsc::channel();
    if send_history_command(HistoryCommand::Snapshot(tx)).is_err() {
        return QueryHistory::new().queries.into();
    }

    rx.recv_timeout(history_writer_response_timeout())
        .unwrap_or_else(|_| QueryHistory::new().queries.into())
}

pub fn history_snapshot() -> Result<Vec<QueryHistoryEntry>, String> {
    let (tx, rx) = mpsc::channel();
    send_history_command(HistoryCommand::Snapshot(tx))
        .map_err(|_| "Failed to fetch query history snapshot".to_string())?;
    rx.recv_timeout(history_writer_response_timeout())
        .map_err(|_| "Failed to fetch query history snapshot".to_string())
}

pub fn clear_history() -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    send_history_command(HistoryCommand::Clear(tx))
        .map_err(|_| "Failed to clear query history".to_string())?;
    rx.recv_timeout(history_writer_response_timeout())
        .map_err(|_| "Timed out while clearing query history".to_string())?
}

/// Query history dialog for viewing and re-executing past queries
pub struct QueryHistoryDialog;

impl QueryHistoryDialog {
    pub fn show_with_registry(popups: Arc<Mutex<Vec<Window>>>) -> Option<String> {
        enum DialogMessage {
            UpdatePreview(usize),
            FilterChanged,
            UseSelected,
            ClearHistory,
            Close,
        }

        let snapshot = load_snapshot();

        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut dialog = Window::default()
            .with_size(800, 500)
            .with_label("Query History");
        center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);

        let mut main_flex = Flex::default().with_pos(10, 10).with_size(780, 480);
        main_flex.set_type(fltk::group::FlexType::Column);
        main_flex.set_spacing(DIALOG_SPACING);

        // Top section with list and preview
        let mut content_flex = Flex::default();
        content_flex.set_type(fltk::group::FlexType::Row);
        content_flex.set_spacing(DIALOG_SPACING);

        // Left - History list
        let mut list_flex = Flex::default();
        list_flex.set_type(fltk::group::FlexType::Column);
        list_flex.set_spacing(DIALOG_SPACING);

        let mut list_label =
            fltk::frame::Frame::default().with_label("Query History (Most Recent First):");
        list_label.set_label_color(theme::text_primary());
        list_flex.fixed(&list_label, LABEL_ROW_HEIGHT);

        let mut filter_row = Flex::default();
        filter_row.set_type(fltk::group::FlexType::Row);
        filter_row.set_spacing(DIALOG_SPACING);

        let mut search_input = Input::default();
        search_input.set_color(theme::input_bg());
        search_input.set_text_color(theme::text_primary());
        search_input.set_tooltip("Filter by SQL text, connection, timestamp, or error");
        filter_row.fixed(&search_input, 224);

        let mut failed_only_check = CheckButton::default().with_label("Failed only");
        failed_only_check.set_label_color(theme::text_primary());
        filter_row.fixed(&failed_only_check, 114);

        filter_row.end();
        list_flex.fixed(&filter_row, INPUT_ROW_HEIGHT);

        let mut browser = HoldBrowser::default();
        browser.set_color(theme::input_bg());
        browser.set_selection_color(theme::selection_strong());
        theme::style_browser_scrollbars(&browser);

        list_flex.end();
        content_flex.fixed(&list_flex, 350);

        // Right - SQL preview
        let mut preview_flex = Flex::default();
        preview_flex.set_type(fltk::group::FlexType::Column);
        preview_flex.set_spacing(DIALOG_SPACING);

        let mut preview_label = fltk::frame::Frame::default().with_label("SQL Preview:");
        preview_label.set_label_color(theme::text_primary());
        preview_flex.fixed(&preview_label, LABEL_ROW_HEIGHT);

        let preview_buffer = TextBuffer::default();
        let preview_style_buffer = TextBuffer::default();
        let mut preview_display = TextDisplay::default();
        preview_display.set_buffer(preview_buffer.clone());
        preview_display.set_color(theme::editor_bg());
        preview_display.set_text_color(theme::text_primary());
        preview_display.set_text_font(configured_editor_profile().normal);
        preview_display.set_text_size(configured_ui_font_size());
        preview_display.set_linenumber_width(48);
        preview_display.set_linenumber_fgcolor(theme::text_muted());
        preview_display.set_linenumber_bgcolor(theme::panel_bg());
        preview_display.set_linenumber_font(configured_editor_profile().normal);
        preview_display.set_linenumber_size(configured_ui_font_size().saturating_sub(2));
        preview_display.set_highlight_data(preview_style_buffer.clone(), preview_style_table());
        theme::style_text_display_scrollbars(&preview_display);
        install_history_text_shortcuts(&mut preview_display, preview_buffer.clone());

        let mut error_label = fltk::frame::Frame::default().with_label("Error details:");
        error_label.set_label_color(theme::text_primary());
        preview_flex.fixed(&error_label, LABEL_ROW_HEIGHT);

        let error_buffer = TextBuffer::default();
        let mut error_display = TextDisplay::default();
        error_display.set_buffer(error_buffer.clone());
        error_display.set_color(theme::panel_alt());
        error_display.set_text_color(theme::text_primary());
        error_display.set_text_font(configured_editor_profile().normal);
        error_display.set_text_size(configured_ui_font_size());
        theme::style_text_display_scrollbars(&error_display);
        install_history_text_shortcuts(&mut error_display, error_buffer.clone());
        error_display.hide();
        error_label.hide();
        preview_flex.fixed(&error_display, 90);

        preview_flex.end();

        content_flex.end();

        // Bottom buttons
        let mut button_flex = Flex::default();
        button_flex.set_type(fltk::group::FlexType::Row);
        button_flex.set_spacing(DIALOG_SPACING);

        let _spacer = fltk::frame::Frame::default();

        let mut use_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Use Query");
        use_btn.set_color(theme::button_primary());
        use_btn.set_label_color(theme::text_primary());
        use_btn.set_frame(FrameType::RFlatBox);

        let mut clear_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Clear History");
        clear_btn.set_color(theme::button_danger());
        clear_btn.set_label_color(theme::text_primary());
        clear_btn.set_frame(FrameType::RFlatBox);

        let mut close_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Close");
        close_btn.set_color(theme::button_subtle());
        close_btn.set_label_color(theme::text_primary());
        close_btn.set_frame(FrameType::RFlatBox);

        button_flex.fixed(&use_btn, BUTTON_WIDTH_LARGE);
        button_flex.fixed(&clear_btn, BUTTON_WIDTH_LARGE);
        button_flex.fixed(&close_btn, BUTTON_WIDTH);
        button_flex.end();
        main_flex.fixed(&button_flex, BUTTON_ROW_HEIGHT);

        main_flex.end();
        dialog.end();
        fltk::group::Group::set_current(current_group.as_ref());

        popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(dialog.clone());
        // State for selected query
        let selected_sql: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let queries: Arc<Mutex<Vec<QueryHistoryEntry>>> = Arc::new(Mutex::new(snapshot));
        let filtered_indices: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

        {
            let query_snapshot = queries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            populate_history_browser(
                &query_snapshot,
                &mut browser,
                &filtered_indices,
                &search_input.value(),
                failed_only_check.value(),
            );
        }

        let (sender, receiver) = mpsc::channel::<DialogMessage>();

        // Browser selection callback - update preview
        let sender_for_preview = sender.clone();
        browser.set_callback(move |b| {
            let selected = b.value();
            if selected > 0 {
                if let Ok(idx) = (selected - 1).try_into() {
                    let _ = sender_for_preview.send(DialogMessage::UpdatePreview(idx));
                    app::awake();
                }
            }
        });

        let sender_for_filter = sender.clone();
        search_input.set_callback(move |_| {
            let _ = sender_for_filter.send(DialogMessage::FilterChanged);
            app::awake();
        });

        let sender_for_failed_only = sender.clone();
        failed_only_check.set_callback(move |_| {
            let _ = sender_for_failed_only.send(DialogMessage::FilterChanged);
            app::awake();
        });

        // Use Query button
        let sender_for_use = sender.clone();
        use_btn.set_callback(move |_| {
            let _ = sender_for_use.send(DialogMessage::UseSelected);
            app::awake();
        });

        // Clear History button
        let sender_for_clear = sender.clone();
        clear_btn.set_callback(move |_| {
            let _ = sender_for_clear.send(DialogMessage::ClearHistory);
            app::awake();
        });

        // Close button
        let sender_for_close = sender;
        close_btn.set_callback(move |_| {
            let _ = sender_for_close.send(DialogMessage::Close);
            app::awake();
        });

        dialog.show();

        let mut preview_buffer = preview_buffer;
        let mut preview_style_buffer = preview_style_buffer;
        let mut error_buffer = error_buffer;
        let mut error_display = error_display.clone();
        let mut error_label = error_label.clone();
        let preview_flex_for_error = preview_flex.clone();
        let mut browser = browser.clone();
        while dialog.shown() {
            fltk::app::wait();
            while let Ok(message) = receiver.try_recv() {
                match message {
                    DialogMessage::UpdatePreview(index) => {
                        let entry_index = {
                            let filtered = filtered_indices
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            filtered.get(index).copied()
                        };
                        let queries = queries
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if let Some(entry) = entry_index.and_then(|idx| queries.get(idx)) {
                            preview_buffer.set_text(&entry.sql);
                            let styles = build_preview_styles(&entry.sql, entry.error_line);
                            if !set_preview_style_buffer(
                                &mut preview_style_buffer,
                                &entry.sql,
                                &styles,
                            ) {
                                set_text_buffer_raw_bytes(&mut preview_style_buffer, &[]);
                            }
                            if entry.success {
                                error_buffer.set_text("");
                                error_display.hide();
                                error_label.hide();
                            } else if let Some(message) = &entry.error_message {
                                error_buffer.set_text(message);
                                error_display.show();
                                error_label.show();
                            } else {
                                error_buffer.set_text("Unknown error");
                                error_display.show();
                                error_label.show();
                            }
                            preview_flex_for_error.layout();
                        }
                    }
                    DialogMessage::FilterChanged => {
                        let query_snapshot = queries
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        populate_history_browser(
                            &query_snapshot,
                            &mut browser,
                            &filtered_indices,
                            &search_input.value(),
                            failed_only_check.value(),
                        );
                        preview_buffer.set_text("");
                        set_text_buffer_raw_bytes(&mut preview_style_buffer, &[]);
                        error_buffer.set_text("");
                        error_display.hide();
                        error_label.hide();
                        preview_flex_for_error.layout();
                    }
                    DialogMessage::UseSelected => {
                        let selected = browser.value();
                        if selected > 0 {
                            if let Ok(idx) = usize::try_from(selected - 1) {
                                let entry_index = {
                                    let filtered = filtered_indices
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    filtered.get(idx).copied()
                                };
                                let queries = queries
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if let Some(entry) =
                                    entry_index.and_then(|actual| queries.get(actual))
                                {
                                    *selected_sql
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                        Some(entry.sql.clone());
                                    dialog.hide();
                                }
                            }
                        } else {
                            fltk::dialog::alert_default("Please select a query from the list");
                        }
                    }
                    DialogMessage::ClearHistory => {
                        let choice = fltk::dialog::choice2_default(
                            "Are you sure you want to clear all query history?",
                            "Cancel",
                            "Clear All",
                            "",
                        );
                        if choice == Some(1) {
                            match clear_history() {
                                Ok(()) => {
                                    app::awake();
                                    queries
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .clear();
                                    let query_snapshot = queries
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    populate_history_browser(
                                        &query_snapshot,
                                        &mut browser,
                                        &filtered_indices,
                                        &search_input.value(),
                                        failed_only_check.value(),
                                    );
                                    preview_buffer.set_text("");
                                    set_text_buffer_raw_bytes(&mut preview_style_buffer, &[]);
                                    error_buffer.set_text("");
                                    error_display.hide();
                                    error_label.hide();
                                    preview_flex_for_error.layout();
                                }
                                Err(err) => {
                                    fltk::dialog::alert_default(&format!(
                                        "Failed to clear query history: {}",
                                        err
                                    ));
                                }
                            }
                        }
                    }
                    DialogMessage::Close => {
                        dialog.hide();
                    }
                }
            }
        }

        // Remove dialog from popups to prevent memory leak
        popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|w| w.as_widget_ptr() != dialog.as_widget_ptr());

        // Explicitly destroy top-level dialog widgets to release native resources.
        Window::delete(dialog);

        let result = selected_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        result
    }

    /// Add a query to history
    pub fn add_to_history(
        sql: &str,
        execution_time_ms: u64,
        row_count: usize,
        connection_name: &str,
        success: bool,
        message: &str,
    ) -> Result<(), String> {
        // Keep UI responsive for large SQL text (for example package body DDL):
        // enqueue raw data and persist on the background history writer.
        let entry = PendingHistoryEntry {
            sql: sql.to_string(),
            timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            execution_time_ms,
            row_count,
            connection_name: connection_name.to_string(),
            success,
            message: message.to_string(),
        };

        if send_history_command(HistoryCommand::Add(entry)).is_err() {
            app::awake();
            return Err("Failed to add query history entry".to_string());
        }
        Ok(())
    }
}

/// Truncate SQL for display in list.
///
/// All indices are byte offsets; `is_char_boundary` is used before slicing so
/// multi-byte characters are never split.
fn truncate_sql(sql: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }

    if max_len <= 3 {
        // Truncate at a char boundary so we don't produce invalid UTF-8.
        let mut end = max_len.min(sql.len());
        while end > 0 && !sql.is_char_boundary(end) {
            end -= 1;
        }
        return sql[..end].to_string();
    }

    let visible_len = max_len - 3;
    for (visible_chars, (byte_idx, _ch)) in sql.char_indices().enumerate() {
        if visible_chars == visible_len {
            let mut output = String::with_capacity(byte_idx + 3);
            output.push_str(&sql[..byte_idx]);
            output.push_str("...");
            return output;
        }
    }

    sql.to_string()
}

/// Case-insensitive substring search. Uses a fast byte-level comparison when
/// the haystack is ASCII (common for SQL, timestamps, connection names),
/// falling back to Unicode case folding only when necessary.
fn contains_lower(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle_lower.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        h.windows(n.len()).any(|w| {
            w.iter()
                .zip(n.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
        })
    } else {
        fold_for_case_insensitive(haystack).contains(needle_lower)
    }
}

fn history_entry_matches_filter(
    entry: &QueryHistoryEntry,
    search_lower: &str,
    failed_only: bool,
) -> bool {
    if failed_only && entry.success {
        return false;
    }

    if search_lower.is_empty() {
        return true;
    }

    if contains_lower(&entry.sql, search_lower) {
        return true;
    }

    if contains_lower(&entry.connection_name, search_lower) {
        return true;
    }

    if contains_lower(&entry.timestamp, search_lower) {
        return true;
    }

    match entry.error_message.as_deref() {
        Some(message) => contains_lower(message, search_lower),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_shortcut_action_accepts_current_ascii_key() {
        assert_eq!(
            resolve_history_text_shortcut_action(
                Key::from_char('c'),
                Key::from_char('x'),
                Shortcut::Ctrl,
            ),
            Some(HistoryTextShortcutAction::Copy)
        );
    }

    #[test]
    fn history_shortcut_action_accepts_original_ascii_key_under_hangul_layout() {
        assert_eq!(
            resolve_history_text_shortcut_action(
                Key::from_char('ㅁ'),
                Key::from_char('a'),
                Shortcut::Ctrl,
            ),
            Some(HistoryTextShortcutAction::SelectAll)
        );
        assert_eq!(
            resolve_history_text_shortcut_action(
                Key::from_char('ㅊ'),
                Key::from_char('c'),
                Shortcut::Ctrl,
            ),
            Some(HistoryTextShortcutAction::Copy)
        );
    }
}

fn history_entry_display(entry: &QueryHistoryEntry) -> String {
    let color_prefix = if entry.success { "@C255 " } else { "@C1 " };
    let sql_first_line = entry
        .sql
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    format!(
        "{color_prefix}{} | {} | {}ms | {} rows",
        entry.timestamp,
        truncate_sql(sql_first_line, 50),
        entry.execution_time_ms,
        entry.row_count
    )
}

fn populate_history_browser(
    entries: &[QueryHistoryEntry],
    browser: &mut HoldBrowser,
    filtered_indices: &Arc<Mutex<Vec<usize>>>,
    search_text: &str,
    failed_only: bool,
) {
    let search_lower = fold_for_case_insensitive(search_text.trim());
    browser.clear();

    let mut indices = filtered_indices
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    indices.clear();

    for (index, entry) in entries.iter().enumerate() {
        if history_entry_matches_filter(entry, &search_lower, failed_only) {
            browser.add(&history_entry_display(entry));
            indices.push(index);
        }
    }
}

#[cfg(test)]
mod query_history_tests {
    use super::{
        build_preview_styles, contains_lower, history_entry_matches_filter,
        materialize_history_entry, parse_error_line, truncate_sql, PendingHistoryEntry,
        QueryHistoryEntry,
    };
    use crate::ui::syntax_highlight::encode_fltk_style_bytes;

    #[test]
    fn truncate_sql_preserves_original_whitespace() {
        let sql = "  SELECT\t'프로시저 테스트'\nFROM dual  ";
        assert_eq!(
            truncate_sql(sql, 100),
            "  SELECT\t'프로시저 테스트'\nFROM dual  "
        );
    }

    #[test]
    fn truncate_sql_truncates_on_char_boundary_for_multibyte_text() {
        let sql = "가나다라마바사";
        assert_eq!(truncate_sql(sql, 5), "가나...");
    }

    #[test]
    fn truncate_sql_does_not_collapse_whitespace_runs() {
        let sql = "SELECT\n\n\t\t*\t\tFROM\t\tdual";
        assert_eq!(truncate_sql(sql, 100), "SELECT\n\n\t\t*\t\tFROM\t\tdual");
    }

    #[test]
    fn truncate_sql_returns_empty_for_zero_limit() {
        assert_eq!(truncate_sql("SELECT 1", 0), "");
    }

    #[test]
    fn parse_error_line_prefers_primary_error_line_reference() {
        let message = "failed near line 1
ORA-06512: at line 27";
        assert_eq!(parse_error_line(message), Some(1));
    }

    #[test]
    fn parse_error_line_ignores_non_error_line_wording() {
        let message = "client command line 8 received
server location at line 12";
        assert_eq!(parse_error_line(message), Some(12));
    }

    #[test]
    fn preview_style_encoding_preserves_multibyte_byte_length() {
        let sql = "SELECT '한글'\nFROM dual";
        let logical_styles = build_preview_styles(sql, Some(1));
        let encoded = encode_fltk_style_bytes(sql, &logical_styles).unwrap_or_default();

        assert_eq!(logical_styles.len(), sql.len());
        assert_eq!(encoded.len(), sql.len());
        assert_eq!(
            encoded.get(8).copied(),
            logical_styles.as_bytes().get(8).copied()
        );
        assert_eq!(encoded.get(9).copied(), Some(0));
        assert_eq!(encoded.get(10).copied(), Some(0));
    }

    #[test]
    fn contains_lower_ascii_fast_path() {
        assert!(contains_lower("SELECT * FROM EMPLOYEES", "employees"));
        assert!(contains_lower("HRDEV", "hrdev"));
        assert!(!contains_lower("HRDEV", "orders"));
        assert!(contains_lower("", ""));
        assert!(!contains_lower("", "x"));
    }

    #[test]
    fn contains_lower_unicode_fallback() {
        assert!(contains_lower("SELECT '프로시저' FROM dual", "프로시저"));
        assert!(!contains_lower("SELECT '프로시저' FROM dual", "테스트"));
    }

    #[test]
    fn history_filter_matches_sql_and_connection() {
        let entry = QueryHistoryEntry {
            sql: "SELECT * FROM employees".to_string(),
            timestamp: "2026-02-21 10:00:00".to_string(),
            execution_time_ms: 10,
            row_count: 1,
            connection_name: "HRDEV".to_string(),
            success: true,
            error_message: None,
            error_line: None,
        };
        assert!(history_entry_matches_filter(&entry, "employees", false));
        assert!(history_entry_matches_filter(&entry, "hrdev", false));
        assert!(!history_entry_matches_filter(&entry, "orders", false));
    }

    #[test]
    fn history_filter_failed_only_excludes_success_rows() {
        let success_entry = QueryHistoryEntry {
            sql: "SELECT 1".to_string(),
            timestamp: "2026-02-21 10:00:00".to_string(),
            execution_time_ms: 5,
            row_count: 1,
            connection_name: "DEV".to_string(),
            success: true,
            error_message: None,
            error_line: None,
        };
        let failed_entry = QueryHistoryEntry {
            sql: "BROKEN".to_string(),
            timestamp: "2026-02-21 11:00:00".to_string(),
            execution_time_ms: 5,
            row_count: 0,
            connection_name: "DEV".to_string(),
            success: false,
            error_message: Some("ORA-00900 invalid SQL statement".to_string()),
            error_line: Some(1),
        };
        assert!(!history_entry_matches_filter(&success_entry, "", true));
        assert!(history_entry_matches_filter(&failed_entry, "", true));
    }

    #[test]
    fn materialize_history_entry_preserves_sql_and_extracts_error_line() {
        let entry = PendingHistoryEntry {
            sql: "CONNECT scott/tiger@localhost:1521/ORCL".to_string(),
            timestamp: "2026-02-27 10:00:00".to_string(),
            execution_time_ms: 21,
            row_count: 0,
            connection_name: "DEV".to_string(),
            success: false,
            message: "ORA-00900 invalid SQL statement near line 7".to_string(),
        };

        let materialized = materialize_history_entry(entry);
        assert_eq!(materialized.sql, "CONNECT scott/tiger@localhost:1521/ORCL");
        assert_eq!(materialized.error_line, Some(7));
        assert!(materialized.error_message.is_some());
    }
}
