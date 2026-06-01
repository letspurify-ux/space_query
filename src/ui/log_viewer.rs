use fltk::{
    app,
    browser::HoldBrowser,
    button::Button,
    enums::FrameType,
    group::Flex,
    menu::Choice,
    prelude::*,
    text::{TextBuffer, TextDisplay},
    window::Window,
};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::theme;
use crate::ui::{configured_editor_profile, configured_ui_font_size};
use crate::utils::logging::{self, LogEntry, LogLevel};

enum DialogMessage {
    UpdatePreview(usize),
    FilterChanged,
    ClearLog,
    ExportLog,
    Close,
}

pub struct LogViewerDialog;

impl LogViewerDialog {
    pub fn show(popups: Arc<Mutex<Vec<Window>>>) {
        let all_entries = logging::get_log_entries();

        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut dialog = Window::default()
            .with_size(900, 560)
            .with_label("Application Log");
        center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);

        let mut main_flex = Flex::default().with_pos(10, 10).with_size(880, 540);
        main_flex.set_type(fltk::group::FlexType::Column);
        main_flex.set_spacing(DIALOG_SPACING);

        // -- Filter row --
        let mut filter_row = Flex::default();
        filter_row.set_type(fltk::group::FlexType::Row);
        filter_row.set_spacing(DIALOG_SPACING);

        let mut filter_label = fltk::frame::Frame::default().with_label("Filter Level:");
        filter_label.set_label_color(theme::text_primary());
        filter_row.fixed(&filter_label, 80);

        let mut level_choice = Choice::default();
        level_choice.set_color(theme::input_bg());
        level_choice.set_text_color(theme::text_primary());
        level_choice.add_choice("All|Error|Warning|Info|Debug");
        level_choice.set_value(0);
        filter_row.fixed(&level_choice, 120);

        let mut count_label = fltk::frame::Frame::default();
        count_label.set_label_color(theme::text_muted());
        count_label.set_align(fltk::enums::Align::Left | fltk::enums::Align::Inside);

        filter_row.end();
        main_flex.fixed(&filter_row, INPUT_ROW_HEIGHT);

        // -- Content: list + detail --
        let mut content_flex = Flex::default();
        content_flex.set_type(fltk::group::FlexType::Row);
        content_flex.set_spacing(DIALOG_SPACING);

        // Left - Log list
        let mut list_flex = Flex::default();
        list_flex.set_type(fltk::group::FlexType::Column);
        list_flex.set_spacing(DIALOG_SPACING);

        let mut list_label =
            fltk::frame::Frame::default().with_label("Log Entries (Most Recent First):");
        list_label.set_label_color(theme::text_primary());
        list_flex.fixed(&list_label, LABEL_ROW_HEIGHT);

        let mut browser = HoldBrowser::default();
        browser.set_color(theme::input_bg());
        browser.set_selection_color(theme::selection_strong());
        theme::style_browser_scrollbars(&browser);

        list_flex.end();
        content_flex.fixed(&list_flex, 420);

        // Right - Detail view
        let mut detail_flex = Flex::default();
        detail_flex.set_type(fltk::group::FlexType::Column);
        detail_flex.set_spacing(DIALOG_SPACING);

        let mut detail_label = fltk::frame::Frame::default().with_label("Details:");
        detail_label.set_label_color(theme::text_primary());
        detail_flex.fixed(&detail_label, LABEL_ROW_HEIGHT);

        let detail_buffer = TextBuffer::default();
        let mut detail_display = TextDisplay::default();
        detail_display.set_buffer(detail_buffer.clone());
        detail_display.set_color(theme::editor_bg());
        detail_display.set_text_color(theme::text_primary());
        detail_display.set_text_font(configured_editor_profile().normal);
        detail_display.set_text_size(configured_ui_font_size());
        detail_display.wrap_mode(fltk::text::WrapMode::AtBounds, 0);
        theme::style_text_display_scrollbars(&detail_display);

        detail_flex.end();

        content_flex.end();

        // -- Bottom buttons --
        let mut button_flex = Flex::default();
        button_flex.set_type(fltk::group::FlexType::Row);
        button_flex.set_spacing(DIALOG_SPACING);

        let _spacer = fltk::frame::Frame::default();

        let mut export_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Export...");
        export_btn.set_color(theme::button_primary());
        export_btn.set_label_color(theme::text_primary());
        export_btn.set_frame(FrameType::RFlatBox);

        let mut clear_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Clear Log");
        clear_btn.set_color(theme::button_danger());
        clear_btn.set_label_color(theme::text_primary());
        clear_btn.set_frame(FrameType::RFlatBox);

        let mut close_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Close");
        close_btn.set_color(theme::button_subtle());
        close_btn.set_label_color(theme::text_primary());
        close_btn.set_frame(FrameType::RFlatBox);

        button_flex.fixed(&export_btn, BUTTON_WIDTH_LARGE);
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

        let entries: Arc<Mutex<Vec<LogEntry>>> = Arc::new(Mutex::new(all_entries));
        let filtered_indices: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

        let (sender, receiver) = mpsc::channel::<DialogMessage>();

        // Initial population
        populate_browser(
            &entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            &mut browser,
            &filtered_indices,
            &mut count_label,
            None,
        );

        // Browser selection callback
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

        // Level filter callback
        let sender_for_filter = sender.clone();
        level_choice.set_callback(move |_| {
            let _ = sender_for_filter.send(DialogMessage::FilterChanged);
            app::awake();
        });

        // Export button
        let sender_for_export = sender.clone();
        export_btn.set_callback(move |_| {
            let _ = sender_for_export.send(DialogMessage::ExportLog);
            app::awake();
        });

        // Clear button
        let sender_for_clear = sender.clone();
        clear_btn.set_callback(move |_| {
            let _ = sender_for_clear.send(DialogMessage::ClearLog);
            app::awake();
        });

        // Close button
        let sender_for_close = sender;
        close_btn.set_callback(move |_| {
            let _ = sender_for_close.send(DialogMessage::Close);
            app::awake();
        });

        dialog.show();

        let mut detail_buffer = detail_buffer;
        let mut browser = browser.clone();
        let mut count_label = count_label.clone();
        let level_choice = level_choice.clone();

        while dialog.shown() {
            fltk::app::wait();
            while let Ok(message) = receiver.try_recv() {
                match message {
                    DialogMessage::UpdatePreview(browser_index) => {
                        let entry_index = {
                            let fi = filtered_indices
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            fi.get(browser_index).copied()
                        };
                        let detail = entry_index.and_then(|entry_index| {
                            let ents = entries
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            ents.get(entry_index).map(|entry| {
                                format!(
                                    "Timestamp: {}\nLevel: {}\nSource: {}\n\n{}",
                                    entry.timestamp,
                                    entry.level.label(),
                                    entry.source,
                                    entry.message
                                )
                            })
                        });
                        if let Some(detail) = detail {
                            detail_buffer.set_text(&detail);
                        }
                    }
                    DialogMessage::FilterChanged => {
                        let filter = selected_filter(&level_choice);
                        populate_browser(
                            &entries
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            &mut browser,
                            &filtered_indices,
                            &mut count_label,
                            filter,
                        );
                        detail_buffer.set_text("");
                    }
                    DialogMessage::ClearLog => {
                        let choice = fltk::dialog::choice2_default(
                            "Are you sure you want to clear all application logs?",
                            "Cancel",
                            "Clear All",
                            "",
                        );
                        if choice == Some(1) {
                            match logging::clear_log() {
                                Ok(()) => {
                                    entries
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .clear();
                                    filtered_indices
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .clear();
                                    browser.clear();
                                    detail_buffer.set_text("");
                                    count_label.set_label("0 entries");
                                }
                                Err(err) => {
                                    fltk::dialog::alert_default(&format!(
                                        "Failed to clear application log: {}",
                                        err
                                    ));
                                }
                            }
                        }
                    }
                    DialogMessage::ExportLog => {
                        let mut dlg = fltk::dialog::FileDialog::new(
                            fltk::dialog::FileDialogType::BrowseSaveFile,
                        );
                        dlg.set_filter("Text Files\t*.txt\nAll Files\t*.*");
                        dlg.show();
                        let path = dlg.filename();
                        if !path.as_os_str().is_empty() {
                            let filtered_snapshot = {
                                filtered_indices
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .clone()
                            };
                            let output = {
                                let ents = entries
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let mut output = String::new();
                                for idx in filtered_snapshot {
                                    if let Some(entry) = ents.get(idx) {
                                        output.push_str(&format!(
                                            "[{}] [{}] [{}] {}\n",
                                            entry.timestamp,
                                            entry.level.label(),
                                            entry.source,
                                            entry.message
                                        ));
                                    }
                                }
                                output
                            };
                            match std::fs::write(&path, output) {
                                Ok(()) => {
                                    fltk::dialog::message_default(&format!(
                                        "Log exported to {}",
                                        path.display()
                                    ));
                                }
                                Err(err) => {
                                    fltk::dialog::alert_default(&format!(
                                        "Failed to export log: {}",
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

        popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|w| w.as_widget_ptr() != dialog.as_widget_ptr());

        // Explicitly destroy top-level dialog widgets to release native resources.
        Window::delete(dialog);
    }
}

fn selected_filter(choice: &Choice) -> Option<LogLevel> {
    match choice.value() {
        1 => Some(LogLevel::Error),
        2 => Some(LogLevel::Warning),
        3 => Some(LogLevel::Info),
        4 => Some(LogLevel::Debug),
        _ => None, // 0 = All
    }
}

fn populate_browser(
    entries: &[LogEntry],
    browser: &mut HoldBrowser,
    filtered_indices: &Arc<Mutex<Vec<usize>>>,
    count_label: &mut fltk::frame::Frame,
    filter: Option<LogLevel>,
) {
    browser.clear();
    let mut indices = Vec::with_capacity(entries.len());

    for (i, entry) in entries.iter().enumerate() {
        if let Some(level) = filter {
            if entry.level != level {
                continue;
            }
        }

        let color_prefix = match entry.level {
            LogLevel::Error => "@C1 ",    // red
            LogLevel::Warning => "@C95 ", // orange
            LogLevel::Info => "@C255 ",   // white
            LogLevel::Debug => "@C246 ",  // gray
        };

        let short_msg = truncate_message(&entry.message, 60);
        let escaped_source = escape_browser_label(&entry.source);
        let escaped_msg = escape_browser_label(&short_msg);
        let display = format!(
            "{}{} [{}] [{}] {}",
            color_prefix,
            entry.timestamp,
            entry.level.label(),
            escaped_source,
            escaped_msg
        );
        browser.add(&display);
        indices.push(i);
    }

    count_label.set_label(&format!("{} entries", indices.len()));
    *filtered_indices
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = indices;
}

fn escape_browser_label(text: &str) -> String {
    text.replace('@', "@@")
}

fn truncate_message(msg: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }

    let mut normalized = String::with_capacity(msg.len());
    let mut last_was_whitespace = true;
    for ch in msg.chars() {
        if ch.is_whitespace() {
            if !last_was_whitespace {
                normalized.push(' ');
                last_was_whitespace = true;
            }
        } else {
            normalized.push(ch);
            last_was_whitespace = false;
        }
    }

    while normalized.ends_with(' ') {
        normalized.pop();
    }

    if normalized.is_empty() {
        return String::new();
    }

    if max_len <= 3 {
        let keep = max_len.min(3);
        return "...".chars().take(keep).collect();
    }

    let visible_len = max_len - 3;
    let mut visible_chars = 0usize;
    let mut truncate_at = normalized.len();
    for (idx, _) in normalized.char_indices() {
        if visible_chars == visible_len {
            truncate_at = idx;
            break;
        }
        visible_chars += 1;
    }

    if visible_chars < visible_len {
        normalized
    } else {
        let mut output = String::with_capacity(truncate_at + 3);
        output.push_str(&normalized[..truncate_at]);
        output.push_str("...");
        output
    }
}

#[cfg(test)]
mod log_viewer_tests {
    use super::*;

    #[test]
    fn truncate_message_handles_long_text() {
        let msg = "a".repeat(100);
        let result = truncate_message(&msg, 50);
        assert!(result.ends_with("..."));
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn truncate_message_respects_tiny_limits() {
        assert_eq!(truncate_message("abcdef", 0), "");
        assert_eq!(truncate_message("abcdef", 1), ".");
        assert_eq!(truncate_message("abcdef", 2), "..");
        assert_eq!(truncate_message("abcdef", 3), "...");
    }

    #[test]
    fn truncate_message_preserves_short_text() {
        let msg = "short message";
        assert_eq!(truncate_message(msg, 50), "short message");
    }

    #[test]
    fn truncate_message_normalizes_whitespace() {
        let msg = "line1\nline2\ttab";
        assert_eq!(truncate_message(msg, 100), "line1 line2 tab");
    }

    #[test]
    fn truncate_message_handles_multibyte() {
        let msg = "가나다라마바사아자차";
        let result = truncate_message(msg, 5);
        assert_eq!(result, "가나...");
    }

    #[test]
    fn escape_browser_label_escapes_fltk_format_prefix() {
        assert_eq!(escape_browser_label("@path/@file"), "@@path/@@file");
    }
}
