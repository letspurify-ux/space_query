use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::text_buffer_access;
use crate::ui::theme;
use fltk::{
    app,
    button::{Button, CheckButton},
    enums::{CallbackTrigger, Event, FrameType, Key, Shortcut},
    group::Flex,
    input::Input,
    prelude::*,
    text::{TextBuffer, TextEditor},
    window::Window,
};
use std::sync::{Arc, Mutex};

fn fold_for_case_insensitive(value: &str) -> String {
    value.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

/// Find/Replace dialog
pub struct FindReplaceDialog;

#[derive(Clone, Default)]
struct FindReplaceSessionState {
    find_text: String,
    replace_text: String,
    case_sensitive: bool,
    search_pos: i32,
    last_search_text: String,
}

thread_local! {
    static FIND_REPLACE_SESSION: Mutex<FindReplaceSessionState> =
        Mutex::new(FindReplaceSessionState::default());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindInputShortcutAction {
    SelectAll,
    Copy,
    Cut,
    Paste,
}

fn normalize_search_pos(text: &str, pos: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let mut p = (pos.max(0) as usize).min(text.len());
    if text.is_char_boundary(p) {
        return p as i32;
    }

    // Clamp invalid UTF-8 byte offsets to the previous valid boundary.
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    p as i32
}

fn resolve_find_start_pos(text: &str, cursor_pos: i32, saved_search_pos: i32) -> i32 {
    let normalized_cursor = normalize_search_pos(text, cursor_pos);
    let normalized_saved = normalize_search_pos(text, saved_search_pos);

    if normalized_cursor != normalized_saved {
        normalized_cursor
    } else {
        normalized_saved
    }
}

fn save_find_replace_state(
    find_input: &Input,
    replace_input: Option<&Input>,
    case_check: &CheckButton,
    search_pos: i32,
    last_search_text: &str,
) {
    FIND_REPLACE_SESSION.with(|state| {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.find_text = find_input.value();
        if let Some(replace_input) = replace_input {
            state.replace_text = replace_input.value();
        }
        state.case_sensitive = case_check.value();
        state.search_pos = search_pos.max(0);
        state.last_search_text = last_search_text.to_string();
    });
}

fn matches_input_shortcut_key(key: Key, original_key: Key, ascii: char) -> bool {
    let lower = Key::from_char(ascii.to_ascii_lowercase());
    let upper = Key::from_char(ascii.to_ascii_uppercase());
    key == lower || key == upper || original_key == lower || original_key == upper
}

fn resolve_find_input_shortcut_action(
    key: Key,
    original_key: Key,
    state: Shortcut,
) -> Option<FindInputShortcutAction> {
    let ctrl_or_cmd = state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Command);
    if !ctrl_or_cmd {
        return None;
    }

    if matches_input_shortcut_key(key, original_key, 'a') {
        return Some(FindInputShortcutAction::SelectAll);
    }
    if matches_input_shortcut_key(key, original_key, 'c') {
        return Some(FindInputShortcutAction::Copy);
    }
    if matches_input_shortcut_key(key, original_key, 'x') {
        return Some(FindInputShortcutAction::Cut);
    }
    if matches_input_shortcut_key(key, original_key, 'v') {
        return Some(FindInputShortcutAction::Paste);
    }

    None
}

fn install_find_input_shortcuts(input: &mut Input) {
    input.handle(move |widget, ev| match ev {
        // Handle only `Event::Shortcut` for Ctrl/Cmd combinations.
        // If we also process `Event::KeyDown`, some environments dispatch both
        // events for the same key press (e.g. paste), causing duplicated input.
        Event::Shortcut => {
            let key = app::event_key();
            let original_key = app::event_original_key();
            let state = app::event_state();

            match resolve_find_input_shortcut_action(key, original_key, state) {
                Some(FindInputShortcutAction::SelectAll) => {
                    let end = widget.value().len().min(i32::MAX as usize) as i32;
                    let _ = widget.set_position(0);
                    let _ = widget.set_mark(end);
                    true
                }
                Some(FindInputShortcutAction::Copy) => widget.copy().is_ok(),
                Some(FindInputShortcutAction::Cut) => widget.cut().is_ok(),
                Some(FindInputShortcutAction::Paste) => {
                    app::paste_text(widget);
                    true
                }
                None => false,
            }
        }
        _ => false,
    });
}

impl FindReplaceDialog {
    pub fn has_search_text() -> bool {
        FIND_REPLACE_SESSION.with(|state| {
            !state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_text
                .is_empty()
        })
    }

    pub fn show_find_with_registry(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        popups: Arc<Mutex<Vec<Window>>>,
    ) {
        Self::show_dialog(editor, buffer, false, popups);
    }

    /// Show find and replace dialog
    pub fn show_replace_with_registry(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        popups: Arc<Mutex<Vec<Window>>>,
    ) {
        Self::show_dialog(editor, buffer, true, popups);
    }

    fn show_dialog(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        show_replace: bool,
        popups: Arc<Mutex<Vec<Window>>>,
    ) {
        enum DialogMessage {
            FindNext {
                search_text: String,
                case_sensitive: bool,
            },
            Replace {
                search_text: String,
                replace_text: String,
                case_sensitive: bool,
            },
            ReplaceAll {
                search_text: String,
                replace_text: String,
                case_sensitive: bool,
            },
            Close,
        }

        let title = if show_replace {
            "Find and Replace"
        } else {
            "Find"
        };
        let height = if show_replace { 180 } else { 130 };

        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut dialog = Window::default().with_size(450, height).with_label(title);
        center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);

        let mut main_flex = Flex::default().with_pos(10, 10).with_size(430, height - 20);
        main_flex.set_type(fltk::group::FlexType::Column);
        main_flex.set_spacing(DIALOG_SPACING);

        // Find input row
        let mut find_flex = Flex::default();
        find_flex.set_type(fltk::group::FlexType::Row);
        let mut find_label = fltk::frame::Frame::default().with_label("Find:");
        find_label.set_label_color(theme::text_primary());
        find_flex.fixed(&find_label, FORM_LABEL_WIDTH);
        let mut find_input = Input::default();
        find_input.set_color(theme::input_bg());
        find_input.set_text_color(theme::text_primary());
        find_input.set_trigger(CallbackTrigger::EnterKeyAlways);
        install_find_input_shortcuts(&mut find_input);
        find_flex.end();
        main_flex.fixed(&find_flex, INPUT_ROW_HEIGHT);

        // Replace input row (if show_replace)
        let replace_input = if show_replace {
            let mut replace_flex = Flex::default();
            replace_flex.set_type(fltk::group::FlexType::Row);
            let mut replace_label = fltk::frame::Frame::default().with_label("Replace:");
            replace_label.set_label_color(theme::text_primary());
            replace_flex.fixed(&replace_label, FORM_LABEL_WIDTH);
            let mut input = Input::default();
            input.set_color(theme::input_bg());
            input.set_text_color(theme::text_primary());
            install_find_input_shortcuts(&mut input);
            replace_flex.end();
            main_flex.fixed(&replace_flex, INPUT_ROW_HEIGHT);
            Some(input)
        } else {
            None
        };

        // Options row
        let mut options_flex = Flex::default();
        options_flex.set_type(fltk::group::FlexType::Row);
        let mut case_check = CheckButton::default().with_label("Case sensitive");
        case_check.set_label_color(theme::text_secondary());
        options_flex.end();
        main_flex.fixed(&options_flex, CHECKBOX_ROW_HEIGHT);

        // Buttons row
        let mut button_flex = Flex::default();
        button_flex.set_type(fltk::group::FlexType::Row);
        button_flex.set_spacing(DIALOG_SPACING);

        let _spacer = fltk::frame::Frame::default();

        let mut find_next_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Find Next");
        find_next_btn.set_color(theme::button_primary());
        find_next_btn.set_label_color(theme::text_primary());
        find_next_btn.set_frame(FrameType::RFlatBox);

        let replace_btn = if show_replace {
            let mut btn = Button::default()
                .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
                .with_label("Replace");
            btn.set_color(theme::button_secondary());
            btn.set_label_color(theme::text_primary());
            btn.set_frame(FrameType::RFlatBox);
            button_flex.fixed(&btn, BUTTON_WIDTH);
            Some(btn)
        } else {
            None
        };

        let replace_all_btn = if show_replace {
            let mut btn = Button::default()
                .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
                .with_label("Replace All");
            btn.set_color(theme::button_secondary());
            btn.set_label_color(theme::text_primary());
            btn.set_frame(FrameType::RFlatBox);
            button_flex.fixed(&btn, BUTTON_WIDTH);
            Some(btn)
        } else {
            None
        };

        let mut close_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Close");
        close_btn.set_color(theme::button_subtle());
        close_btn.set_label_color(theme::text_primary());
        close_btn.set_frame(FrameType::RFlatBox);

        button_flex.fixed(&find_next_btn, BUTTON_WIDTH);
        button_flex.fixed(&close_btn, BUTTON_WIDTH_SMALL);
        button_flex.end();
        main_flex.fixed(&button_flex, BUTTON_ROW_HEIGHT);

        main_flex.end();
        dialog.end();
        fltk::group::Group::set_current(current_group.as_ref());

        popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(dialog.clone());
        let session_snapshot = FIND_REPLACE_SESSION.with(|state| {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        });

        if !session_snapshot.find_text.is_empty() {
            find_input.set_value(&session_snapshot.find_text);
        }
        case_check.set_value(session_snapshot.case_sensitive);

        if let Some(mut replace_input_widget) = replace_input.clone() {
            if !session_snapshot.replace_text.is_empty() {
                replace_input_widget.set_value(&session_snapshot.replace_text);
            }
        }

        // State for search
        let initial_search_pos = resolve_find_start_pos(
            &buffer.text(),
            editor.insert_position(),
            session_snapshot.search_pos,
        );
        let search_pos = Arc::new(Mutex::new(initial_search_pos));
        let last_search_text = Arc::new(Mutex::new(session_snapshot.last_search_text));

        let (sender, receiver) = std::sync::mpsc::channel::<DialogMessage>();

        // Find Next callback
        let sender_for_find = sender.clone();
        let find_input_clone = find_input.clone();
        let case_check_clone = case_check.clone();
        find_next_btn.set_callback(move |_| {
            let search_text = find_input_clone.value();
            if search_text.is_empty() {
                return;
            }

            let _ = sender_for_find.send(DialogMessage::FindNext {
                search_text,
                case_sensitive: case_check_clone.value(),
            });
            app::awake();
        });

        // Enter key in find input triggers Find Next
        let sender_for_find_enter = sender.clone();
        let find_input_enter = find_input.clone();
        let case_check_enter = case_check.clone();
        find_input.set_callback(move |_| {
            let search_text = find_input_enter.value();
            if search_text.is_empty() {
                return;
            }
            let _ = sender_for_find_enter.send(DialogMessage::FindNext {
                search_text,
                case_sensitive: case_check_enter.value(),
            });
            app::awake();
        });

        // Replace callback
        if let Some(mut replace_btn) = replace_btn {
            if let Some(replace_input_clone) = replace_input.clone() {
                let sender_for_replace = sender.clone();
                let find_input_clone = find_input.clone();
                let case_check_clone = case_check.clone();

                replace_btn.set_callback(move |_| {
                    let search_text = find_input_clone.value();
                    let replace_text = replace_input_clone.value();

                    if search_text.is_empty() {
                        return;
                    }

                    let _ = sender_for_replace.send(DialogMessage::Replace {
                        search_text,
                        replace_text,
                        case_sensitive: case_check_clone.value(),
                    });
                    app::awake();
                });
            } else {
                eprintln!("Replace input not available for replace action.");
            }
        }

        // Replace All callback
        if let Some(mut replace_all_btn) = replace_all_btn {
            if let Some(replace_input_clone) = replace_input.clone() {
                let sender_for_replace_all = sender.clone();
                let find_input_clone = find_input.clone();
                let case_check_clone = case_check.clone();

                replace_all_btn.set_callback(move |_| {
                    let search_text = find_input_clone.value();
                    let replace_text = replace_input_clone.value();

                    if search_text.is_empty() {
                        return;
                    }

                    let _ = sender_for_replace_all.send(DialogMessage::ReplaceAll {
                        search_text,
                        replace_text,
                        case_sensitive: case_check_clone.value(),
                    });
                    app::awake();
                });
            } else {
                eprintln!("Replace input not available for replace-all action.");
            }
        }

        // Close callback
        let sender_for_close = sender;
        close_btn.set_callback(move |_| {
            let _ = sender_for_close.send(DialogMessage::Close);
            app::awake();
        });

        dialog.show();

        let mut buffer = buffer.clone();
        let mut editor = editor.clone();
        let find_input_state = find_input.clone();
        let replace_input_state = replace_input;
        let case_check_state = case_check.clone();
        let search_pos_state = search_pos.clone();
        let last_search_text_state = last_search_text.clone();

        while dialog.shown() {
            fltk::app::wait();
            while let Ok(message) = receiver.try_recv() {
                match message {
                    DialogMessage::FindNext {
                        search_text,
                        case_sensitive,
                    } => {
                        if *last_search_text
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            != search_text
                        {
                            *search_pos
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
                            *last_search_text
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                search_text.clone();
                        }
                        let text = buffer.text();
                        let saved_search_pos = *search_pos
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let start_pos = resolve_find_start_pos(
                            &text,
                            editor.insert_position(),
                            saved_search_pos,
                        );
                        *search_pos
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = start_pos;

                        if let Some((match_start, match_end)) =
                            find_next_match(&text, &search_text, start_pos, case_sensitive)
                        {
                            buffer.select(match_start as i32, match_end as i32);
                            editor.set_insert_position(match_end as i32);
                            editor.show_insert_position();
                            // Use match_end instead of match_start + 1 to avoid UTF-8 boundary issues
                            *search_pos
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                match_end.min(text.len()) as i32;
                        } else if start_pos > 0 {
                            if let Some((match_start, match_end)) =
                                find_next_match(&text, &search_text, 0, case_sensitive)
                            {
                                buffer.select(match_start as i32, match_end as i32);
                                editor.set_insert_position(match_end as i32);
                                editor.show_insert_position();
                                *search_pos
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    match_end.min(text.len()) as i32;
                                fltk::dialog::message_default(
                                    "Reached end, searching from beginning...",
                                );
                            } else {
                                *search_pos
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
                                fltk::dialog::message_default("Text not found");
                            }
                        } else {
                            fltk::dialog::message_default("Text not found");
                        }
                    }
                    DialogMessage::Replace {
                        search_text,
                        replace_text,
                        case_sensitive,
                    } => {
                        if *last_search_text
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            != search_text
                        {
                            *last_search_text
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                search_text.clone();
                        }
                        if let Some((start, end)) = buffer.selection_position() {
                            let selected =
                                text_buffer_access::text_range(&buffer, None, start, end);
                            let matches = if case_sensitive {
                                selected == search_text
                            } else {
                                fold_for_case_insensitive(&selected)
                                    == fold_for_case_insensitive(&search_text)
                            };

                            if matches {
                                buffer.remove(start, end);
                                buffer.insert(start, &replace_text);
                                let next_pos = start + replace_text.len() as i32;
                                editor.set_insert_position(next_pos);
                                *search_pos
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    normalize_search_pos(&buffer.text(), next_pos);
                            }
                        }
                    }
                    DialogMessage::ReplaceAll {
                        search_text,
                        replace_text,
                        case_sensitive,
                    } => {
                        if *last_search_text
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            != search_text
                        {
                            *last_search_text
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                search_text.clone();
                        }
                        if search_text.is_empty() {
                            fltk::dialog::message_default("Search text is empty");
                            continue;
                        }
                        let text = buffer.text();
                        let new_text = if case_sensitive {
                            text.replace(&search_text, &replace_text)
                        } else {
                            let mut result = String::with_capacity(text.len());
                            let mut search_pos = 0usize;
                            while let Some((match_start, match_end)) =
                                find_next_match(&text, &search_text, search_pos as i32, false)
                            {
                                result.push_str(text.get(search_pos..match_start).unwrap_or(""));
                                result.push_str(&replace_text);
                                search_pos = match_end;
                                if search_pos >= text.len() {
                                    break;
                                }
                            }
                            result.push_str(text.get(search_pos..).unwrap_or(""));
                            result
                        };

                        let count = if case_sensitive {
                            text.matches(&search_text).count()
                        } else {
                            let mut count = 0usize;
                            let mut search_pos = 0usize;
                            while let Some((_match_start, match_end)) =
                                find_next_match(&text, &search_text, search_pos as i32, false)
                            {
                                count += 1;
                                search_pos = match_end;
                                if search_pos >= text.len() {
                                    break;
                                }
                            }
                            count
                        };

                        buffer.set_text(&new_text);
                        *search_pos
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = 0;
                        fltk::dialog::message_default(&format!("Replaced {} occurrences", count));
                    }
                    DialogMessage::Close => {
                        save_find_replace_state(
                            &find_input_state,
                            replace_input_state.as_ref(),
                            &case_check_state,
                            *search_pos_state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            &last_search_text_state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        );
                        dialog.hide();
                    }
                }

                if dialog.shown() {
                    save_find_replace_state(
                        &find_input_state,
                        replace_input_state.as_ref(),
                        &case_check_state,
                        *search_pos_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        &last_search_text_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                    );
                }
            }
        }

        save_find_replace_state(
            &find_input_state,
            replace_input_state.as_ref(),
            &case_check_state,
            *search_pos_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            &last_search_text_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );

        // Remove dialog from popups to prevent memory leak
        popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|w| w.as_widget_ptr() != dialog.as_widget_ptr());

        // Explicitly destroy top-level dialog widgets to release native resources.
        Window::delete(dialog);
    }

    pub fn find_next_from_session(editor: &mut TextEditor, buffer: &mut TextBuffer) -> bool {
        let session = FIND_REPLACE_SESSION.with(|state| {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        });
        if session.find_text.is_empty() {
            return false;
        }

        let text = buffer.text();
        let start_pos = if session.last_search_text != session.find_text {
            normalize_search_pos(&text, editor.insert_position())
        } else {
            resolve_find_start_pos(&text, editor.insert_position(), session.search_pos)
        };

        let found = find_next_match(&text, &session.find_text, start_pos, session.case_sensitive)
            .or_else(|| {
                if start_pos > 0 {
                    find_next_match(&text, &session.find_text, 0, session.case_sensitive)
                } else {
                    None
                }
            });

        if let Some((match_start, match_end)) = found {
            buffer.select(match_start as i32, match_end as i32);
            editor.set_insert_position(match_end as i32);
            editor.show_insert_position();
            FIND_REPLACE_SESSION.with(|state| {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.last_search_text = session.find_text.clone();
                state.search_pos = match_end as i32;
            });
            true
        } else {
            false
        }
    }

    /// Find next occurrence (for F3 shortcut)
    #[allow(dead_code)]
    pub fn find_next(
        editor: &mut TextEditor,
        buffer: &mut TextBuffer,
        search_text: &str,
        case_sensitive: bool,
    ) -> bool {
        if search_text.is_empty() {
            return false;
        }

        let current_pos = editor.insert_position();
        let text = buffer.text();
        let start_pos = normalize_search_pos(&text, current_pos);

        if let Some((match_start, match_end)) =
            find_next_match(&text, search_text, start_pos, case_sensitive)
        {
            buffer.select(match_start as i32, match_end as i32);
            editor.set_insert_position(match_end as i32);
            editor.show_insert_position();
            true
        } else {
            // Try from beginning
            if let Some((match_start, match_end)) =
                find_next_match(&text, search_text, 0, case_sensitive)
            {
                buffer.select(match_start as i32, match_end as i32);
                editor.set_insert_position(match_end as i32);
                editor.show_insert_position();
                true
            } else {
                false
            }
        }
    }
}

fn find_next_match(
    text: &str,
    search_text: &str,
    start_pos: i32,
    case_sensitive: bool,
) -> Option<(usize, usize)> {
    if search_text.is_empty() || text.is_empty() {
        return None;
    }
    let start_pos = normalize_search_pos(text, start_pos) as usize;
    let haystack = text.get(start_pos..)?;
    if case_sensitive {
        let pos = haystack.find(search_text)?;
        let match_start = start_pos + pos;
        let match_end = match_start + search_text.len();
        return Some((match_start, match_end));
    }

    let (relative_start, relative_end) =
        find_unicode_case_insensitive_bounds(haystack, search_text)?;
    Some((start_pos + relative_start, start_pos + relative_end))
}

fn find_unicode_case_insensitive_bounds(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let needle_folded = fold_for_case_insensitive(needle);
    if needle_folded.is_empty() {
        return None;
    }

    let needle_folded_len = needle_folded.len();

    for (start, _) in haystack.char_indices() {
        let mut folded = String::new();
        for (relative_idx, ch) in haystack[start..].char_indices() {
            folded.extend(ch.to_lowercase());
            if folded.len() == needle_folded_len && folded == needle_folded {
                let end = start + relative_idx + ch.len_utf8();
                return Some((start, end));
            }
            if folded.len() >= needle_folded_len {
                break;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        find_next_match, normalize_search_pos, resolve_find_input_shortcut_action,
        resolve_find_start_pos, FindInputShortcutAction,
    };
    use fltk::enums::{Key, Shortcut};

    #[test]
    fn normalize_search_pos_clamps_non_boundary_utf8_offset() {
        let text = "ab한글cd";
        let mid_char_offset = text.find('한').expect("expected utf-8 anchor") + 1;
        let normalized = normalize_search_pos(text, mid_char_offset as i32);
        assert_eq!(normalized as usize, text.find('한').unwrap_or(0));
    }

    #[test]
    fn resolve_find_start_pos_prefers_current_cursor_over_stale_saved_position() {
        let text = "alpha beta gamma";
        let cursor_pos = text.find("gamma").unwrap_or(0) as i32;
        let stale_saved_pos = text.find("beta").unwrap_or(0) as i32;

        let resolved = resolve_find_start_pos(text, cursor_pos, stale_saved_pos);

        assert_eq!(resolved, cursor_pos);
    }

    #[test]
    fn find_input_shortcut_action_accepts_current_ascii_key() {
        assert_eq!(
            resolve_find_input_shortcut_action(
                Key::from_char('c'),
                Key::from_char('x'),
                Shortcut::Ctrl,
            ),
            Some(FindInputShortcutAction::Copy)
        );
    }

    #[test]
    fn find_input_shortcut_action_accepts_original_ascii_key_under_hangul_layout() {
        assert_eq!(
            resolve_find_input_shortcut_action(
                Key::from_char('ㅁ'),
                Key::from_char('a'),
                Shortcut::Ctrl,
            ),
            Some(FindInputShortcutAction::SelectAll)
        );
        assert_eq!(
            resolve_find_input_shortcut_action(
                Key::from_char('ㅊ'),
                Key::from_char('c'),
                Shortcut::Ctrl,
            ),
            Some(FindInputShortcutAction::Copy)
        );
        assert_eq!(
            resolve_find_input_shortcut_action(
                Key::from_char('ㅌ'),
                Key::from_char('x'),
                Shortcut::Ctrl,
            ),
            Some(FindInputShortcutAction::Cut)
        );
        assert_eq!(
            resolve_find_input_shortcut_action(
                Key::from_char('ㅍ'),
                Key::from_char('v'),
                Shortcut::Ctrl,
            ),
            Some(FindInputShortcutAction::Paste)
        );
    }

    #[test]
    fn find_next_match_clamps_non_boundary_utf8_offset() {
        let text = "a한b한c";
        let second_han = text.rfind('한').expect("expected second utf8 anchor");
        let mid_second_han = second_han + 1;
        let (start, end) = find_next_match(text, "한", mid_second_han as i32, true)
            .expect("expected to find second match");
        assert_eq!(start, second_han);
        assert_eq!(end, second_han + "한".len());
    }

    #[test]
    fn find_next_match_case_insensitive_handles_unicode_letters() {
        let text = "Ärger ärger";
        let first = find_next_match(text, "ärger", 0, false).expect("expected first match");
        assert_eq!(&text[first.0..first.1], "Ärger");

        let second =
            find_next_match(text, "ÄRGER", first.1 as i32, false).expect("expected second match");
        assert_eq!(&text[second.0..second.1], "ärger");
    }

    #[test]
    fn find_next_match_case_insensitive_handles_utf8_with_byte_scan() {
        let text = "가A나a";
        let first_ascii = text.find('A').expect("expected first ascii letter");
        let second_ascii = text.rfind('a').expect("expected second ascii letter");

        let first = find_next_match(text, "a", 0, false).expect("expected first match");
        assert_eq!(first, (first_ascii, first_ascii + "A".len()));

        let second =
            find_next_match(text, "A", first.1 as i32, false).expect("expected second match");
        assert_eq!(second, (second_ascii, second_ascii + "a".len()));
    }

    #[test]
    fn find_next_match_case_insensitive_scales_for_long_ascii_text() {
        let mut text = "x".repeat(20_000);
        text.push_str("if.");

        let found = find_next_match(&text, "IF.", 0, false).expect("expected to find match");
        assert_eq!(found, (20_000, 20_003));
    }
}
