use fltk::{
    app,
    button::Button,
    enums::{FrameType, Shortcut},
    menu::{MenuBar, MenuFlag},
    prelude::*,
    text::{TextBuffer, TextDisplay, WrapMode},
    window::Window,
};
use std::{collections::HashMap, path::PathBuf};

use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::theme;
use crate::ui::{configured_editor_profile, configured_ui_font_size};
use crate::utils::arithmetic::safe_div;
use crate::utils::config::MAX_RECENT_SQL_FILES;

pub struct MenuBarBuilder;

fn forward_menu_callback(menu: &mut MenuBar) {
    menu.do_callback();
}

fn show_info_dialog(title: &str, content: &str, width: i32, height: i32) {
    let current_group = fltk::group::Group::try_current();

    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let mut dialog = Window::default().with_size(width, height).with_label(title);
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);
    dialog.begin();

    let mut display = TextDisplay::default()
        .with_pos(10, 10)
        .with_size(width - 20, height - 60);
    display.set_color(theme::editor_bg());
    display.set_text_color(theme::text_primary());
    display.set_text_font(configured_editor_profile().normal);
    display.set_text_size(configured_ui_font_size());
    display.wrap_mode(WrapMode::AtBounds, 0);
    theme::style_text_display_scrollbars(&display);

    let mut buffer = TextBuffer::default();
    buffer.set_text(content);
    display.set_buffer(buffer);

    let button_x = safe_div(width - BUTTON_WIDTH, 2);
    let button_y = height - BUTTON_HEIGHT - DIALOG_MARGIN;
    let mut close_btn = Button::default()
        .with_pos(button_x, button_y)
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Close");
    close_btn.set_color(theme::button_secondary());
    close_btn.set_label_color(theme::text_primary());
    close_btn.set_frame(FrameType::RFlatBox);

    let mut dialog_handle = dialog.clone();
    close_btn.set_callback(move |_| {
        dialog_handle.hide();
        app::awake();
    });

    dialog.end();
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    while dialog.shown() {
        app::wait();
    }

    // Explicitly destroy top-level dialog widgets to release native resources.
    Window::delete(dialog);
}

fn build_about_dialog_content() -> String {
    let version = crate::version::display_version();
    let build_profile = if cfg!(debug_assertions) {
        "Debug"
    } else {
        "Release"
    };
    let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);

    format!(
        "SPACE Query\n\
Version {version}\n\
\n\
Desktop SQL client for Oracle and MySQL/MariaDB built with Rust and FLTK.\n\
\n\
Highlights\n\
- Multi-tab SQL editor with execution history and result/message tabs\n\
- Oracle and MySQL/MariaDB object browser, syntax highlighting, and IntelliSense\n\
- Automatic SQL formatting for Oracle SQL, PL/SQL, and MySQL scripts\n\
- Explain Plan / EXPLAIN, SQL*Plus-style script execution, and transaction controls\n\
- Saved connections, OS keyring password storage, and application log viewer\n\
\n\
Copyright (c) 2026 SPACE Query contributors\n\
Licensed under MIT OR Apache-2.0\n\
\n\
Oracle and MySQL are trademarks of Oracle and/or its affiliates.\n\
This project is not affiliated with or endorsed by Oracle.\n\
\n\
Third-Party Software\n\
- TNS thin client: based on python-oracledb (Apache-2.0) and go-ora (MIT).\n\
  See THIRD_PARTY_NOTICES.md.\n\
- Rust dependency licenses: see THIRD_PARTY_DEPENDENCIES.md.\n\
\n\
Runtime\n\
- Build: {build_profile}\n\
- Platform: {platform}"
    )
}

impl MenuBarBuilder {
    pub fn build() -> MenuBar {
        Self::build_with_recent_sql_files(&[])
    }

    pub fn build_with_recent_sql_files(recent_sql_files: &[PathBuf]) -> MenuBar {
        let mut menu = MenuBar::default();
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        menu.set_id("main_menu");
        Self::populate(&mut menu, recent_sql_files);
        menu
    }

    pub fn sync_recent_sql_file_items(menu: &mut MenuBar, recent_sql_files: &[PathBuf]) {
        menu.clear();
        Self::populate(menu, recent_sql_files);
        menu.redraw();
    }

    pub fn recent_sql_file_choice_index(choice: &str) -> Option<usize> {
        let label = choice.strip_prefix("File/").unwrap_or(choice);
        Self::recent_sql_file_label_index(label)
    }

    pub fn recent_sql_file_choice_map(recent_sql_files: &[PathBuf]) -> HashMap<String, PathBuf> {
        let mut choices = HashMap::new();
        for (slot, path) in recent_sql_files
            .iter()
            .take(MAX_RECENT_SQL_FILES)
            .enumerate()
        {
            let slot_label = format!("Recent {}", slot + 1);
            let plain_label = format!(
                "{}: {}",
                slot_label,
                Self::recent_sql_file_display_name(path)
            );
            let escaped_label = Self::recent_sql_file_menu_label(slot, path);
            for label in [slot_label, plain_label, escaped_label] {
                choices.insert(label.clone(), path.clone());
                choices.insert(format!("File/{label}"), path.clone());
            }
        }
        choices
    }

    fn recent_sql_file_label_index(label: &str) -> Option<usize> {
        let rest = label.strip_prefix("Recent ")?;
        let digits_len = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .map(char::len_utf8)
            .sum::<usize>();
        if digits_len == 0 {
            return None;
        }
        let slot = rest[..digits_len].parse::<usize>().ok()?;
        if !(1..=MAX_RECENT_SQL_FILES).contains(&slot) {
            return None;
        }
        Some(slot - 1)
    }

    fn escape_menu_label(label: &str) -> String {
        let mut escaped = String::with_capacity(label.len());
        for ch in label.chars() {
            if matches!(ch, '&' | '/' | '\\' | '_') {
                escaped.push('\\');
            }
            escaped.push(ch);
        }
        escaped
    }

    fn recent_sql_file_display_name(path: &std::path::Path) -> String {
        path.file_name()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string())
    }

    fn recent_sql_file_menu_label(slot: usize, path: &std::path::Path) -> String {
        Self::escape_menu_label(&format!(
            "Recent {}: {}",
            slot + 1,
            Self::recent_sql_file_display_name(path)
        ))
    }

    fn add_recent_sql_file_slots(menu: &mut MenuBar, recent_sql_files: &[PathBuf]) {
        for (slot, path) in recent_sql_files
            .iter()
            .take(MAX_RECENT_SQL_FILES)
            .enumerate()
        {
            let label = Self::recent_sql_file_menu_label(slot, path);
            menu.add(
                &format!("&File/{label}"),
                Shortcut::None,
                MenuFlag::Normal,
                forward_menu_callback,
            );
        }
    }

    fn populate(menu: &mut MenuBar, recent_sql_files: &[PathBuf]) {
        // File menu
        menu.add(
            "&File/&Connect",
            Shortcut::Command | 'n',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/&Disconnect",
            Shortcut::Command | 'd',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&File/&New SQL File",
            Shortcut::Command | 't',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/&Open SQL File",
            Shortcut::Command | 'o',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/&Save SQL File",
            Shortcut::Command | 's',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/Save SQL File &As",
            Shortcut::Command | Shortcut::Shift | 's',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/&Close SQL File",
            Shortcut::Command | 'w',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&File/E&xit",
            Shortcut::Command | 'q',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&File/ ",
            Shortcut::None,
            MenuFlag::Inactive | MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        Self::add_recent_sql_file_slots(menu, recent_sql_files);

        // Edit menu
        menu.add(
            "&Edit/&Undo",
            Shortcut::Command | 'z',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Redo",
            Shortcut::Command | 'y',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Cu&t",
            Shortcut::Command | 'x',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Copy",
            Shortcut::Command | 'c',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Copy with &Headers",
            Shortcut::Command | Shortcut::Shift | 'c',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Paste",
            Shortcut::Command | 'v',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Select &All",
            Shortcut::Command | 'a',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Find",
            Shortcut::Command | 'f',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Find &Next",
            Shortcut::from_key(fltk::enums::Key::F3),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Replace",
            Shortcut::Command | 'h',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Format SQL",
            Shortcut::Command | Shortcut::Shift | 'f',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Toggle &Comment",
            Shortcut::Command | '/',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Upper&case Selection",
            Shortcut::Command | 'u',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/Lower&case Selection",
            Shortcut::Command | 'l',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Edit/&Intellisense",
            Shortcut::Command | ' ',
            MenuFlag::Normal,
            forward_menu_callback,
        );

        // Query menu
        menu.add(
            "&Query/&Execute",
            Shortcut::from_key(fltk::enums::Key::F5),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/Execute &Statement",
            Shortcut::Ctrl | fltk::enums::Key::Enter,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/Execute Statement (&F9)",
            Shortcut::from_key(fltk::enums::Key::F9),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/Execute &Selected",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/&Quick Describe",
            Shortcut::from_key(fltk::enums::Key::F4),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Query/E&xplain Plan",
            Shortcut::from_key(fltk::enums::Key::F6),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Query/&Commit",
            Shortcut::from_key(fltk::enums::Key::F7),
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Query/&Rollback",
            Shortcut::from_key(fltk::enums::Key::F8),
            MenuFlag::Normal,
            forward_menu_callback,
        );

        // Tools menu
        menu.add(
            "&Tools/&Refresh Objects",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/&Export Results",
            Shortcut::Command | 'e',
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/&Query History",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/&Session Activity",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/Application &Log",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/",
            Shortcut::None,
            MenuFlag::MenuDivider,
            forward_menu_callback,
        );
        menu.add(
            "&Tools/&Auto-Commit",
            Shortcut::None,
            MenuFlag::Toggle,
            forward_menu_callback,
        );

        // Settings menu
        menu.add(
            "&Settings/&Preferences",
            Shortcut::None,
            MenuFlag::Normal,
            forward_menu_callback,
        );

        // Help menu
        menu.add("&Help/&About", Shortcut::None, MenuFlag::Normal, |_| {
            let content = build_about_dialog_content();
            show_info_dialog("About", &content, 640, 430);
        });
        menu.add(
            "&Help/&Keyboard Shortcuts",
            Shortcut::None,
            MenuFlag::Normal,
            |_| {
                show_info_dialog(
                    "Keyboard Shortcuts",
                    "Keyboard Shortcuts:\n\n\
                    macOS note: use Cmd where Ctrl is shown.\n\n\
                    File:\n\
                    Ctrl+N - Connect\n\
                    Ctrl+D - Disconnect\n\
                    Ctrl+T - New SQL File\n\
                    Ctrl+O - Open SQL File\n\
                    Ctrl+S - Save SQL File\n\
                    Ctrl+Shift+S - Save SQL File As\n\
                    Ctrl+W - Close SQL File\n\
                    Ctrl+Q - Exit\n\n\
                    Edit (SQL Editor):\n\
                    Ctrl+Z - Undo\n\
                    Ctrl+Y - Redo\n\
                    Ctrl+X - Cut\n\
                    Ctrl+C - Copy\n\
                    Ctrl+Shift+C - Copy with Headers\n\
                    Ctrl+V - Paste\n\
                    Ctrl+A - Select All\n\
                    Ctrl+F - Find\n\
                    F3 - Find Next\n\
                    Ctrl+H - Replace\n\
                    Ctrl+Shift+F - Format SQL\n\
                    Ctrl+/ - Toggle Comment\n\
                    Ctrl+U - Uppercase Selection\n\
                    Ctrl+L - Lowercase Selection\n\
                    Ctrl+Space - Intellisense\n\
                    Ctrl+Shift+Up/Down - Select SQL Block\n\
                    Ctrl+Click - Quick Describe at Cursor\n\n\
                    Query:\n\
                    Ctrl+Enter - Execute Statement\n\
                    F5 - Execute Script\n\
                    F9 - Execute Statement\n\
                    F6 - Explain Plan\n\
                    F7 - Commit\n\
                    F8 - Rollback\n\
                    F4 - Quick Describe (Editor)\n\n\
                    Tools:\n\
                    Ctrl+E - Export Results\n\
                    Query History - no shortcut\n\n\
                    Results Table:\n\
                    Ctrl+C - Copy Selected Cells\n\
                    Ctrl+Shift+C - Copy with Headers\n\
                    Ctrl+A - Select All\n\n\
                    Object Browser:\n\
                    Enter - Generate SELECT (tables/views)",
                    640,
                    640,
                );
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::MenuBarBuilder;

    #[test]
    fn recent_sql_file_choice_index_reads_numbered_file_menu_items() {
        assert_eq!(
            MenuBarBuilder::recent_sql_file_choice_index("File/Recent 1: query.sql"),
            Some(0)
        );
        assert_eq!(
            MenuBarBuilder::recent_sql_file_choice_index("File/Recent 10: query.sql"),
            Some(9)
        );
        assert_eq!(
            MenuBarBuilder::recent_sql_file_choice_index("File/Recent 11: query.sql"),
            None
        );
        assert_eq!(
            MenuBarBuilder::recent_sql_file_choice_index("File/Recent 7: test\\_all.txt"),
            Some(6)
        );
        assert_eq!(
            MenuBarBuilder::recent_sql_file_choice_index("Recent 7: test\\_all.txt"),
            Some(6)
        );
    }

    #[test]
    fn recent_sql_file_menu_label_escapes_path_separators_and_accelerators() {
        let label =
            MenuBarBuilder::recent_sql_file_menu_label(0, std::path::Path::new("query.sql"));

        assert_eq!(label, r"Recent 1: query.sql");
        assert_eq!(
            MenuBarBuilder::escape_menu_label("a&b/c_d\\e"),
            r"a\&b\/c\_d\\e"
        );
        assert_eq!(
            MenuBarBuilder::recent_sql_file_menu_label(6, std::path::Path::new("test_all.txt")),
            r"Recent 7: test\_all.txt"
        );
    }

    #[test]
    fn recent_sql_file_choice_map_points_menu_labels_to_paths() {
        let files = vec![
            std::path::PathBuf::from("/tmp/test1.txt"),
            std::path::PathBuf::from("/tmp/test_all.txt"),
        ];

        let choices = MenuBarBuilder::recent_sql_file_choice_map(&files);

        assert_eq!(
            choices.get("File/Recent 2: test\\_all.txt"),
            Some(&files[1])
        );
        assert_eq!(choices.get("Recent 2"), Some(&files[1]));
    }
}
