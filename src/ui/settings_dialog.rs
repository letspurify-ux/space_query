use fltk::{
    app,
    browser::HoldBrowser,
    button::Button,
    enums::{Align, CallbackTrigger, FrameType},
    frame::Frame,
    group::{Flex, FlexType, Group, Tabs},
    input::{Input, IntInput},
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

fn fold_for_case_insensitive(value: &str) -> String {
    value.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

use crate::ui::constants::*;
use crate::ui::{available_font_names, center_on_main, theme};
use crate::utils::{
    AppConfig, MAX_CANCEL_TIMEOUT_SECONDS, MAX_CONNECTION_POOL_SIZE, MAX_LAZY_FETCH_BATCH_SIZE,
    MIN_CANCEL_TIMEOUT_SECONDS, MIN_CONNECTION_POOL_SIZE, MIN_LAZY_FETCH_BATCH_SIZE,
};

pub struct FontSettings {
    pub font: String,
    pub ui_size: u32,
    pub editor_size: u32,
    pub result_size: u32,
    pub result_cell_max_chars: u32,
    pub lazy_fetch_batch_size: u32,
    pub connection_pool_size: u32,
    pub cancel_timeout_seconds: u32,
}

fn validate_size(label: &str, value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (8..=48).contains(&size) => Some(size),
        _ => {
            fltk::dialog::alert_default(&format!(
                "{} size must be a number between 8 and 48.",
                label
            ));
            None
        }
    }
}

fn validate_ui_size(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (8..=24).contains(&size) => Some(size),
        _ => {
            fltk::dialog::alert_default("Global UI size must be a number between 8 and 24.");
            None
        }
    }
}

fn validate_result_cell_max_chars(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size)
            if (RESULT_CELL_MAX_DISPLAY_CHARS_MIN..=RESULT_CELL_MAX_DISPLAY_CHARS_MAX)
                .contains(&size) =>
        {
            Some(size)
        }
        _ => {
            fltk::dialog::alert_default(&format!(
                "Cell preview max length must be a number between {} and {}.",
                RESULT_CELL_MAX_DISPLAY_CHARS_MIN, RESULT_CELL_MAX_DISPLAY_CHARS_MAX
            ));
            None
        }
    }
}

fn validate_lazy_fetch_batch_size(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (MIN_LAZY_FETCH_BATCH_SIZE..=MAX_LAZY_FETCH_BATCH_SIZE).contains(&size) => {
            Some(size)
        }
        _ => {
            fltk::dialog::alert_default(&format!(
                "Lazy fetch size must be a number between {} and {}.",
                MIN_LAZY_FETCH_BATCH_SIZE, MAX_LAZY_FETCH_BATCH_SIZE
            ));
            None
        }
    }
}

fn validate_connection_pool_size(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (MIN_CONNECTION_POOL_SIZE..=MAX_CONNECTION_POOL_SIZE).contains(&size) => {
            Some(size)
        }
        _ => {
            fltk::dialog::alert_default(&format!(
                "Connection pool size must be a number between {} and {}.",
                MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE
            ));
            None
        }
    }
}

fn validate_cancel_timeout_seconds(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(seconds)
            if (MIN_CANCEL_TIMEOUT_SECONDS..=MAX_CANCEL_TIMEOUT_SECONDS).contains(&seconds) =>
        {
            Some(seconds)
        }
        _ => {
            fltk::dialog::alert_default(&format!(
                "Cancel timeout must be a number between {} and {} seconds.",
                MIN_CANCEL_TIMEOUT_SECONDS, MAX_CANCEL_TIMEOUT_SECONDS
            ));
            None
        }
    }
}

fn refill_font_list(
    browser: &mut HoldBrowser,
    all_fonts: &[String],
    query: &str,
    filtered: &mut Vec<String>,
    selected_font: &mut String,
) {
    let query = fold_for_case_insensitive(query.trim());

    filtered.clear();
    browser.clear();

    for name in all_fonts {
        if query.is_empty() || fold_for_case_insensitive(name).contains(&query) {
            filtered.push(name.clone());
        }
    }

    for name in filtered.iter() {
        browser.add(name);
    }

    if filtered.is_empty() {
        return;
    }

    let selected_index = filtered
        .iter()
        .position(|name| name.eq_ignore_ascii_case(selected_font))
        .unwrap_or(0);
    browser.select((selected_index + 1) as i32);
    *selected_font = filtered[selected_index].clone();
}

pub fn show_settings_dialog(config: &AppConfig) -> Option<FontSettings> {
    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let mut font_names = available_font_names();
    let current_font = if !config.editor_font.trim().is_empty() {
        config.editor_font.clone()
    } else if !config.result_font.trim().is_empty() {
        config.result_font.clone()
    } else {
        "Courier".to_string()
    };
    if !font_names
        .iter()
        .any(|font_name| font_name.eq_ignore_ascii_case(&current_font))
    {
        font_names.push(current_font.clone());
    }

    let width = 520;
    let height = 560;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Settings");
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let content_margin = DIALOG_MARGIN + 4;
    let content_x = content_margin;
    let content_y = content_margin;
    let content_w = width - content_margin * 2;
    let button_h = BUTTON_ROW_HEIGHT;
    let tabs_h = height - content_margin * 2 - DIALOG_SPACING - button_h;

    let mut tabs = Tabs::new(content_x, content_y, content_w, tabs_h, None);
    tabs.set_color(theme::panel_bg());
    tabs.set_selection_color(theme::selection_strong());
    tabs.set_frame(FrameType::RFlatBox);
    tabs.set_label_color(theme::text_secondary());
    tabs.set_label_size((TAB_HEADER_HEIGHT - 8).max(8));
    tabs.set_tab_align(Align::Center);

    tabs.begin();

    let tab_body_y = content_y + TAB_HEADER_HEIGHT;
    let tab_body_h = (tabs_h - TAB_HEADER_HEIGHT).max(120);

    let mut font_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    font_group.set_label("Font");
    font_group.set_color(theme::panel_bg());
    font_group.set_label_color(theme::text_secondary());
    font_group.set_align(Align::Center | Align::Inside);
    font_group.begin();

    let mut font_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    font_flex.set_type(FlexType::Column);
    font_flex.set_spacing(DIALOG_SPACING);

    let mut search_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    search_row.set_type(FlexType::Row);
    search_row.set_spacing(DIALOG_SPACING);
    let mut search_label = Frame::default().with_label("Search:");
    search_label.set_label_color(theme::text_primary());
    let mut search_input = Input::default();
    search_input.set_color(theme::input_bg());
    search_input.set_text_color(theme::text_primary());
    search_input.set_trigger(CallbackTrigger::Changed);
    search_row.fixed(&search_label, FORM_LABEL_WIDTH);
    search_row.end();
    font_flex.fixed(&search_row, INPUT_ROW_HEIGHT);

    let mut font_browser = HoldBrowser::default().with_size(0, 260);
    font_browser.set_color(theme::input_bg());
    font_browser.set_selection_color(theme::selection_strong());
    theme::style_browser_scrollbars(&font_browser);
    font_flex.resizable(&font_browser);

    let mut selected_row = Flex::default().with_size(0, CHECKBOX_ROW_HEIGHT);
    selected_row.set_type(FlexType::Row);
    selected_row.set_spacing(DIALOG_SPACING);
    let mut selected_label = Frame::default().with_label("Selected:");
    selected_label.set_label_color(theme::text_primary());
    let mut selected_value = Frame::default();
    selected_value.set_label(&current_font);
    selected_value.set_label_color(theme::text_secondary());
    selected_row.fixed(&selected_label, FORM_LABEL_WIDTH);
    selected_row.end();
    font_flex.fixed(&selected_row, CHECKBOX_ROW_HEIGHT);

    let mut editor_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    editor_size_row.set_type(FlexType::Row);
    editor_size_row.set_spacing(DIALOG_SPACING);
    let mut editor_size_label = Frame::default().with_label("Editor:");
    editor_size_label.set_label_color(theme::text_primary());
    editor_size_row.fixed(&editor_size_label, FORM_LABEL_WIDTH);
    let mut editor_size_input = IntInput::default();
    editor_size_input.set_value(&config.editor_font_size.to_string());
    editor_size_input.set_color(theme::input_bg());
    editor_size_input.set_text_color(theme::text_primary());
    editor_size_row.fixed(&editor_size_input, NUMERIC_INPUT_WIDTH);
    let _editor_size_spacer = Frame::default();
    editor_size_row.end();
    font_flex.fixed(&editor_size_row, INPUT_ROW_HEIGHT);

    let mut result_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    result_size_row.set_type(FlexType::Row);
    result_size_row.set_spacing(DIALOG_SPACING);
    let mut result_size_label = Frame::default().with_label("Result Font:");
    result_size_label.set_label_color(theme::text_primary());
    result_size_row.fixed(&result_size_label, FORM_LABEL_WIDTH);
    let mut result_size_input = IntInput::default();
    result_size_input.set_value(&config.result_font_size.to_string());
    result_size_input.set_color(theme::input_bg());
    result_size_input.set_text_color(theme::text_primary());
    result_size_row.fixed(&result_size_input, NUMERIC_INPUT_WIDTH);
    let _result_size_spacer = Frame::default();
    result_size_row.end();
    font_flex.fixed(&result_size_row, INPUT_ROW_HEIGHT);

    let mut global_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    global_size_row.set_type(FlexType::Row);
    global_size_row.set_spacing(DIALOG_SPACING);
    let mut global_size_label = Frame::default().with_label("Global UI:");
    global_size_label.set_label_color(theme::text_primary());
    global_size_row.fixed(&global_size_label, FORM_LABEL_WIDTH);
    let mut global_size_input = IntInput::default();
    global_size_input.set_value(&config.ui_font_size.to_string());
    global_size_input.set_color(theme::input_bg());
    global_size_input.set_text_color(theme::text_primary());
    global_size_row.fixed(&global_size_input, NUMERIC_INPUT_WIDTH);
    let _global_size_spacer = Frame::default();
    global_size_row.end();
    font_flex.fixed(&global_size_row, INPUT_ROW_HEIGHT);

    let mut size_hint = Frame::default().with_label("Font size: 8 ~ 48pt, Global UI: 8 ~ 24pt");
    size_hint.set_label_color(theme::text_secondary());
    font_flex.fixed(&size_hint, LABEL_ROW_HEIGHT);

    font_flex.end();
    font_group.resizable(&font_flex);
    font_group.end();

    let mut result_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    result_group.set_label("Result View");
    result_group.set_color(theme::panel_bg());
    result_group.set_label_color(theme::text_secondary());
    result_group.set_align(Align::Center | Align::Inside);
    result_group.begin();

    let mut result_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    result_flex.set_type(FlexType::Column);
    result_flex.set_spacing(DIALOG_SPACING);

    let mut result_cell_max_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    result_cell_max_row.set_type(FlexType::Row);
    result_cell_max_row.set_spacing(DIALOG_SPACING);
    let mut result_cell_max_label = Frame::default().with_label("Cell Preview:");
    result_cell_max_label.set_label_color(theme::text_primary());
    result_cell_max_row.fixed(&result_cell_max_label, FORM_LABEL_WIDTH);
    let mut result_cell_max_input = IntInput::default();
    result_cell_max_input.set_value(&config.result_cell_max_chars.to_string());
    result_cell_max_input.set_color(theme::input_bg());
    result_cell_max_input.set_text_color(theme::text_primary());
    result_cell_max_row.fixed(&result_cell_max_input, NUMERIC_INPUT_WIDTH);
    let _result_cell_max_spacer = Frame::default();
    result_cell_max_row.end();
    result_flex.fixed(&result_cell_max_row, INPUT_ROW_HEIGHT);

    let mut lazy_fetch_batch_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    lazy_fetch_batch_row.set_type(FlexType::Row);
    lazy_fetch_batch_row.set_spacing(DIALOG_SPACING);
    let mut lazy_fetch_batch_label = Frame::default().with_label("Lazy Fetch:");
    lazy_fetch_batch_label.set_label_color(theme::text_primary());
    lazy_fetch_batch_row.fixed(&lazy_fetch_batch_label, FORM_LABEL_WIDTH);
    let mut lazy_fetch_batch_input = IntInput::default();
    lazy_fetch_batch_input.set_value(&config.normalized_lazy_fetch_batch_size().to_string());
    lazy_fetch_batch_input.set_color(theme::input_bg());
    lazy_fetch_batch_input.set_text_color(theme::text_primary());
    lazy_fetch_batch_row.fixed(&lazy_fetch_batch_input, NUMERIC_INPUT_WIDTH);
    let _lazy_fetch_batch_spacer = Frame::default();
    lazy_fetch_batch_row.end();
    result_flex.fixed(&lazy_fetch_batch_row, INPUT_ROW_HEIGHT);

    let mut preview_hint = Frame::default().with_label(&format!(
        "Cell preview max: {} ~ {} chars",
        RESULT_CELL_MAX_DISPLAY_CHARS_MIN, RESULT_CELL_MAX_DISPLAY_CHARS_MAX
    ));
    preview_hint.set_label_color(theme::text_secondary());
    result_flex.fixed(&preview_hint, LABEL_ROW_HEIGHT);

    let mut lazy_fetch_hint = Frame::default().with_label(&format!(
        "Lazy fetch size: {} ~ {} rows",
        MIN_LAZY_FETCH_BATCH_SIZE, MAX_LAZY_FETCH_BATCH_SIZE
    ));
    lazy_fetch_hint.set_label_color(theme::text_secondary());
    result_flex.fixed(&lazy_fetch_hint, LABEL_ROW_HEIGHT);

    let filler = Frame::default();
    result_flex.resizable(&filler);
    result_flex.end();
    result_group.resizable(&result_flex);
    result_group.end();

    let mut connection_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    connection_group.set_label("Connection");
    connection_group.set_color(theme::panel_bg());
    connection_group.set_label_color(theme::text_secondary());
    connection_group.set_align(Align::Center | Align::Inside);
    connection_group.begin();

    let mut connection_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    connection_flex.set_type(FlexType::Column);
    connection_flex.set_spacing(DIALOG_SPACING);

    let mut pool_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    pool_size_row.set_type(FlexType::Row);
    pool_size_row.set_spacing(DIALOG_SPACING);
    let mut pool_size_label = Frame::default().with_label("Session Pool:");
    pool_size_label.set_label_color(theme::text_primary());
    pool_size_row.fixed(&pool_size_label, FORM_LABEL_WIDTH);
    let mut pool_size_input = IntInput::default();
    pool_size_input.set_value(&config.normalized_connection_pool_size().to_string());
    pool_size_input.set_color(theme::input_bg());
    pool_size_input.set_text_color(theme::text_primary());
    pool_size_row.fixed(&pool_size_input, NUMERIC_INPUT_WIDTH);
    let _pool_size_spacer = Frame::default();
    pool_size_row.end();
    connection_flex.fixed(&pool_size_row, INPUT_ROW_HEIGHT);

    let mut cancel_timeout_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    cancel_timeout_row.set_type(FlexType::Row);
    cancel_timeout_row.set_spacing(DIALOG_SPACING);
    let mut cancel_timeout_label = Frame::default().with_label("Cancel Timeout:");
    cancel_timeout_label.set_label_color(theme::text_primary());
    cancel_timeout_row.fixed(&cancel_timeout_label, FORM_LABEL_WIDTH);
    let mut cancel_timeout_input = IntInput::default();
    cancel_timeout_input.set_value(&config.normalized_cancel_timeout_seconds().to_string());
    cancel_timeout_input.set_color(theme::input_bg());
    cancel_timeout_input.set_text_color(theme::text_primary());
    cancel_timeout_row.fixed(&cancel_timeout_input, NUMERIC_INPUT_WIDTH);
    let _cancel_timeout_spacer = Frame::default();
    cancel_timeout_row.end();
    connection_flex.fixed(&cancel_timeout_row, INPUT_ROW_HEIGHT);

    let mut pool_hint = Frame::default().with_label(&format!(
        "Pool size: {} ~ {} (next connect)",
        MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE
    ));
    pool_hint.set_label_color(theme::text_secondary());
    connection_flex.fixed(&pool_hint, LABEL_ROW_HEIGHT);

    let mut cancel_timeout_hint = Frame::default().with_label(&format!(
        "Cancel timeout: {} ~ {} sec",
        MIN_CANCEL_TIMEOUT_SECONDS, MAX_CANCEL_TIMEOUT_SECONDS
    ));
    cancel_timeout_hint.set_label_color(theme::text_secondary());
    connection_flex.fixed(&cancel_timeout_hint, LABEL_ROW_HEIGHT);

    let connection_filler = Frame::default();
    connection_flex.resizable(&connection_filler);
    connection_flex.end();
    connection_group.resizable(&connection_flex);
    connection_group.end();

    tabs.end();

    let mut button_row = Flex::new(
        content_x,
        content_y + tabs_h + DIALOG_SPACING,
        content_w,
        button_h,
        None,
    );
    button_row.set_type(FlexType::Row);
    button_row.set_spacing(DIALOG_SPACING);
    let btn_spacer = Frame::default();
    button_row.resizable(&btn_spacer);
    let mut cancel_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Cancel");
    cancel_btn.set_color(theme::button_cancel());
    cancel_btn.set_label_color(theme::text_primary());
    cancel_btn.set_frame(FrameType::RFlatBox);
    let mut ok_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Save");
    ok_btn.set_color(theme::button_primary());
    ok_btn.set_label_color(theme::text_primary());
    ok_btn.set_frame(FrameType::RFlatBox);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.fixed(&ok_btn, BUTTON_WIDTH);
    button_row.end();

    dialog.end();
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    let all_fonts = Arc::new(font_names);
    let selected_font = Arc::new(Mutex::new(current_font));
    let filtered_fonts = Arc::new(Mutex::new(Vec::<String>::new()));

    {
        let mut filtered = filtered_fonts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut selected = selected_font
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        refill_font_list(
            &mut font_browser,
            all_fonts.as_ref(),
            "",
            &mut filtered,
            &mut selected,
        );
        selected_value.set_label(&selected);
    }

    let mut font_browser_for_search = font_browser.clone();
    let all_fonts_for_search = all_fonts.clone();
    let filtered_fonts_for_search = filtered_fonts.clone();
    let selected_font_for_search = selected_font.clone();
    let mut selected_value_for_search = selected_value.clone();
    search_input.set_callback(move |input| {
        let mut filtered = filtered_fonts_for_search
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut selected = selected_font_for_search
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        refill_font_list(
            &mut font_browser_for_search,
            all_fonts_for_search.as_ref(),
            &input.value(),
            &mut filtered,
            &mut selected,
        );
        selected_value_for_search.set_label(&selected);
    });

    let selected_font_for_browser = selected_font.clone();
    let mut selected_value_for_browser = selected_value.clone();
    font_browser.set_callback(move |browser| {
        if let Some(name) = browser.selected_text() {
            *selected_font_for_browser
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = name.clone();
            selected_value_for_browser.set_label(&name);
        }
    });

    let result = Arc::new(Mutex::new(None::<FontSettings>));
    let result_for_ok = result.clone();
    let mut dialog_handle = dialog.clone();
    let editor_size_input_ok = editor_size_input.clone();
    let result_size_input_ok = result_size_input.clone();
    let global_size_input_ok = global_size_input.clone();
    let result_cell_max_input_ok = result_cell_max_input.clone();
    let lazy_fetch_batch_input_ok = lazy_fetch_batch_input.clone();
    let pool_size_input_ok = pool_size_input.clone();
    let cancel_timeout_input_ok = cancel_timeout_input.clone();
    let selected_font_ok = selected_font.clone();
    ok_btn.set_callback(move |_| {
        let ui_size = match validate_ui_size(&global_size_input_ok.value()) {
            Some(size) => size,
            None => return,
        };
        let editor_size = match validate_size("Editor", &editor_size_input_ok.value()) {
            Some(size) => size,
            None => return,
        };
        let result_size = match validate_size("Results", &result_size_input_ok.value()) {
            Some(size) => size,
            None => return,
        };
        let result_cell_max_chars =
            match validate_result_cell_max_chars(&result_cell_max_input_ok.value()) {
                Some(size) => size,
                None => return,
            };
        let lazy_fetch_batch_size =
            match validate_lazy_fetch_batch_size(&lazy_fetch_batch_input_ok.value()) {
                Some(size) => size,
                None => return,
            };
        let connection_pool_size = match validate_connection_pool_size(&pool_size_input_ok.value())
        {
            Some(size) => size,
            None => return,
        };
        let cancel_timeout_seconds =
            match validate_cancel_timeout_seconds(&cancel_timeout_input_ok.value()) {
                Some(seconds) => seconds,
                None => return,
            };
        let font = selected_font_ok
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .trim()
            .to_string();
        if font.is_empty() {
            fltk::dialog::alert_default("Please select a font.");
            return;
        }
        *result_for_ok
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(FontSettings {
            font,
            ui_size,
            editor_size,
            result_size,
            result_cell_max_chars,
            lazy_fetch_batch_size,
            connection_pool_size,
            cancel_timeout_seconds,
        });
        dialog_handle.hide();
        app::awake();
    });

    let mut dialog_handle = dialog.clone();
    cancel_btn.set_callback(move |_| {
        dialog_handle.hide();
        app::awake();
    });

    while dialog.shown() {
        app::wait();
    }

    // Explicitly destroy top-level dialog widgets to release native resources.
    Window::delete(dialog);

    let final_result = result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    final_result
}
