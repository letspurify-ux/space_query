use fltk::{
    app,
    browser::HoldBrowser,
    button::Button,
    enums::{Align, CallbackTrigger, FrameType},
    frame::Frame,
    group::{Flex, FlexType, Group, Tabs},
    input::{Input, IntInput},
    menu::Choice,
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

fn fold_for_case_insensitive(value: &str) -> String {
    value.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

use crate::ui::constants::*;
use crate::ui::{available_font_names, center_on_main, resolved_font_name, theme};
use crate::utils::{
    AppConfig, SqlCommaListLayout, MAX_APP_LOG_LIMIT, MAX_CANCEL_TIMEOUT_SECONDS,
    MAX_CONNECTION_POOL_SIZE, MAX_CONNECT_TIMEOUT_SECONDS, MAX_FONT_SIZE,
    MAX_INTELLISENSE_CONTEXT_WINDOW_KIB, MAX_INTELLISENSE_POPUP_DELAY_MS,
    MAX_LAZY_FETCH_BATCH_SIZE, MAX_QUERY_HISTORY_LIMIT, MAX_SQL_FORMAT_RIGHT_MARGIN,
    MAX_UI_FONT_SIZE, MAX_UI_SCALE_PERCENT, MIN_APP_LOG_LIMIT, MIN_CANCEL_TIMEOUT_SECONDS,
    MIN_CONNECTION_POOL_SIZE, MIN_CONNECT_TIMEOUT_SECONDS, MIN_FONT_SIZE,
    MIN_INTELLISENSE_CONTEXT_WINDOW_KIB, MIN_INTELLISENSE_POPUP_DELAY_MS,
    MIN_LAZY_FETCH_BATCH_SIZE, MIN_QUERY_HISTORY_LIMIT, MIN_SQL_FORMAT_RIGHT_MARGIN,
    MIN_UI_FONT_SIZE, MIN_UI_SCALE_PERCENT,
};

#[derive(Clone)]
pub struct FontSettings {
    pub font: String,
    pub ui_size: u32,
    pub ui_scale_percent: u32,
    pub editor_size: u32,
    pub result_size: u32,
    pub result_cell_max_chars: u32,
    pub lazy_fetch_batch_size: u32,
    pub intellisense_context_window_kib: u32,
    pub intellisense_popup_delay_ms: u32,
    pub connection_pool_size: u32,
    pub connect_timeout_seconds: u32,
    pub cancel_timeout_seconds: u32,
    pub sql_comma_list_layout: SqlCommaListLayout,
    pub sql_format_right_margin: u32,
    pub query_history_limit: u32,
    pub app_log_limit: u32,
}

/// Every form label in this dialog, so one column width can fit them all.
const FORM_LABELS: [&str; 13] = [
    "Editor:",
    "Result Font:",
    "Global UI:",
    "Screen Scale:",
    "Cell Preview:",
    "Lazy Fetch:",
    "Context Window:",
    "Popup Delay:",
    "Session Pool:",
    "Connect Timeout:",
    "Cancel Timeout:",
    "Comma Lists:",
    "Right Margin:",
];

/// Width of the label column, wide enough for the longest label.
///
/// A `Frame` centers its label and clips both ends once the text outgrows its
/// box, so a fixed [`FORM_LABEL_WIDTH`] silently ate the first character of
/// `Connect Timeout:` and `Context Window:`. The UI font size is configurable,
/// which is why the column is measured instead of assumed. `Font::Helvetica` is
/// the slot [`apply_global_default_font`](crate::ui::font_settings) remaps to
/// the configured UI font, so it measures what a label will actually draw with.
fn form_label_width() -> i32 {
    fltk::draw::set_font(fltk::enums::Font::Helvetica, fltk::app::font_size());
    FORM_LABELS
        .iter()
        .map(|label| fltk::draw::measure(label, false).0)
        .max()
        .unwrap_or(0)
        .saturating_add(FORM_LABEL_TEXT_GAP)
        .max(FORM_LABEL_WIDTH)
}

/// Breathing room between the widest label and the input beside it.
const FORM_LABEL_TEXT_GAP: i32 = 10;

fn validate_size(label: &str, value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) => Some(size),
        _ => {
            crate::ui::alert_on_main(&format!(
                "{} size must be a number between {} and {}.",
                label, MIN_FONT_SIZE, MAX_FONT_SIZE
            ));
            None
        }
    }
}

fn validate_ui_size(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size) if (MIN_UI_FONT_SIZE..=MAX_UI_FONT_SIZE).contains(&size) => Some(size),
        _ => {
            crate::ui::alert_on_main(&format!(
                "Global UI size must be a number between {} and {}.",
                MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE
            ));
            None
        }
    }
}

fn validate_ui_scale_percent(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(percent) if (MIN_UI_SCALE_PERCENT..=MAX_UI_SCALE_PERCENT).contains(&percent) => {
            Some(percent)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "Screen scale must be a number between {} and {} percent.",
                MIN_UI_SCALE_PERCENT, MAX_UI_SCALE_PERCENT
            ));
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
            crate::ui::alert_on_main(&format!(
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
            crate::ui::alert_on_main(&format!(
                "Lazy fetch size must be a number between {} and {}.",
                MIN_LAZY_FETCH_BATCH_SIZE, MAX_LAZY_FETCH_BATCH_SIZE
            ));
            None
        }
    }
}

fn validate_intellisense_context_window_kib(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(size)
            if (MIN_INTELLISENSE_CONTEXT_WINDOW_KIB..=MAX_INTELLISENSE_CONTEXT_WINDOW_KIB)
                .contains(&size) =>
        {
            Some(size)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "IntelliSense context must be a number between {} and {} KiB.",
                MIN_INTELLISENSE_CONTEXT_WINDOW_KIB, MAX_INTELLISENSE_CONTEXT_WINDOW_KIB
            ));
            None
        }
    }
}

fn validate_intellisense_popup_delay_ms(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(delay_ms)
            if (MIN_INTELLISENSE_POPUP_DELAY_MS..=MAX_INTELLISENSE_POPUP_DELAY_MS)
                .contains(&delay_ms) =>
        {
            Some(delay_ms)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "IntelliSense popup delay must be a number between {} and {} milliseconds.",
                MIN_INTELLISENSE_POPUP_DELAY_MS, MAX_INTELLISENSE_POPUP_DELAY_MS
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
            crate::ui::alert_on_main(&format!(
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
            crate::ui::alert_on_main(&format!(
                "Cancel timeout must be a number between {} and {} seconds.",
                MIN_CANCEL_TIMEOUT_SECONDS, MAX_CANCEL_TIMEOUT_SECONDS
            ));
            None
        }
    }
}

fn validate_connect_timeout_seconds(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(seconds)
            if (MIN_CONNECT_TIMEOUT_SECONDS..=MAX_CONNECT_TIMEOUT_SECONDS).contains(&seconds) =>
        {
            Some(seconds)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "Connect timeout must be a number between {} and {} seconds.",
                MIN_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS
            ));
            None
        }
    }
}

fn validate_sql_format_right_margin(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(margin)
            if (MIN_SQL_FORMAT_RIGHT_MARGIN..=MAX_SQL_FORMAT_RIGHT_MARGIN).contains(&margin) =>
        {
            Some(margin)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "SQL format right margin must be a number between {} and {}.",
                MIN_SQL_FORMAT_RIGHT_MARGIN, MAX_SQL_FORMAT_RIGHT_MARGIN
            ));
            None
        }
    }
}

fn validate_query_history_limit(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(limit) if (MIN_QUERY_HISTORY_LIMIT..=MAX_QUERY_HISTORY_LIMIT).contains(&limit) => {
            Some(limit)
        }
        _ => {
            crate::ui::alert_on_main(&format!(
                "Query history size must be a number between {} and {}.",
                MIN_QUERY_HISTORY_LIMIT, MAX_QUERY_HISTORY_LIMIT
            ));
            None
        }
    }
}

fn validate_app_log_limit(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(limit) if (MIN_APP_LOG_LIMIT..=MAX_APP_LOG_LIMIT).contains(&limit) => Some(limit),
        _ => {
            crate::ui::alert_on_main(&format!(
                "Application log size must be a number between {} and {}.",
                MIN_APP_LOG_LIMIT, MAX_APP_LOG_LIMIT
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

    let font_names = available_font_names();
    let configured_font = if !config.editor_font.trim().is_empty() {
        config.editor_font.as_str()
    } else if !config.result_font.trim().is_empty() {
        config.result_font.as_str()
    } else {
        "Courier"
    };
    let current_font = resolved_font_name(configured_font);

    // Wide enough for every tab label to stay readable in the header.
    let width = 660;
    let height = 560 + INPUT_ROW_HEIGHT + DIALOG_SPACING;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Settings");
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    // One column width for every tab, so the inputs line up across them and no
    // label is clipped at the configured UI font size.
    let label_width = form_label_width();

    let content_margin = DIALOG_MARGIN + 4;
    let content_x = content_margin;
    let content_y = content_margin;
    let content_w = width - content_margin * 2;
    let button_h = BUTTON_ROW_HEIGHT;
    let tabs_h = height - content_margin * 2 - DIALOG_SPACING - button_h;

    let mut tabs = Tabs::new(content_x, content_y, content_w, tabs_h, None);
    tabs.set_color(theme::panel_bg());
    tabs.set_selection_color(theme::selection_soft());
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
    theme::apply_text_input_inset(&mut search_input);
    search_input.set_trigger(CallbackTrigger::Changed);
    search_row.fixed(&search_label, label_width);
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
    if !current_font.eq_ignore_ascii_case(configured_font) {
        selected_value.set_tooltip(&format!(
            "Configured font '{}' is unavailable; '{}' is being used.",
            configured_font, current_font
        ));
    }
    selected_value.set_label_color(theme::text_secondary());
    selected_value.set_align(Align::Left | Align::Inside);
    selected_row.fixed(&selected_label, label_width);
    selected_row.end();
    font_flex.fixed(&selected_row, CHECKBOX_ROW_HEIGHT);

    let mut editor_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    editor_size_row.set_type(FlexType::Row);
    editor_size_row.set_spacing(DIALOG_SPACING);
    let mut editor_size_label = Frame::default().with_label("Editor:");
    editor_size_label.set_label_color(theme::text_primary());
    editor_size_row.fixed(&editor_size_label, label_width);
    let mut editor_size_input = IntInput::default();
    editor_size_input.set_value(&config.normalized_editor_font_size().to_string());
    editor_size_input.set_color(theme::input_bg());
    editor_size_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut editor_size_input);
    editor_size_row.end();
    font_flex.fixed(&editor_size_row, INPUT_ROW_HEIGHT);

    let mut result_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    result_size_row.set_type(FlexType::Row);
    result_size_row.set_spacing(DIALOG_SPACING);
    let mut result_size_label = Frame::default().with_label("Result Font:");
    result_size_label.set_label_color(theme::text_primary());
    result_size_row.fixed(&result_size_label, label_width);
    let mut result_size_input = IntInput::default();
    result_size_input.set_value(&config.normalized_result_font_size().to_string());
    result_size_input.set_color(theme::input_bg());
    result_size_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut result_size_input);
    result_size_row.end();
    font_flex.fixed(&result_size_row, INPUT_ROW_HEIGHT);

    let mut global_size_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    global_size_row.set_type(FlexType::Row);
    global_size_row.set_spacing(DIALOG_SPACING);
    let mut global_size_label = Frame::default().with_label("Global UI:");
    global_size_label.set_label_color(theme::text_primary());
    global_size_row.fixed(&global_size_label, label_width);
    let mut global_size_input = IntInput::default();
    global_size_input.set_value(&config.normalized_ui_font_size().to_string());
    global_size_input.set_color(theme::input_bg());
    global_size_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut global_size_input);
    global_size_row.end();
    font_flex.fixed(&global_size_row, INPUT_ROW_HEIGHT);

    let mut ui_scale_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    ui_scale_row.set_type(FlexType::Row);
    ui_scale_row.set_spacing(DIALOG_SPACING);
    let mut ui_scale_label = Frame::default().with_label("Screen Scale:");
    ui_scale_label.set_label_color(theme::text_primary());
    ui_scale_row.fixed(&ui_scale_label, label_width);
    let mut ui_scale_input = IntInput::default();
    ui_scale_input.set_value(&config.normalized_ui_scale_percent().to_string());
    ui_scale_input.set_color(theme::input_bg());
    ui_scale_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut ui_scale_input);
    ui_scale_input.set_tooltip("Application screen scale percentage");
    let mut ui_scale_unit = Frame::default().with_label("%");
    ui_scale_unit.set_label_color(theme::text_secondary());
    ui_scale_row.fixed(&ui_scale_unit, 24);
    ui_scale_row.end();
    font_flex.fixed(&ui_scale_row, INPUT_ROW_HEIGHT);

    let mut size_hint = Frame::default().with_label(&format!(
        "Font: {} ~ {}pt, Global UI: {} ~ {}pt, Scale: {} ~ {}%",
        MIN_FONT_SIZE,
        MAX_FONT_SIZE,
        MIN_UI_FONT_SIZE,
        MAX_UI_FONT_SIZE,
        MIN_UI_SCALE_PERCENT,
        MAX_UI_SCALE_PERCENT
    ));
    size_hint.set_label_color(theme::text_secondary());
    font_flex.fixed(&size_hint, LABEL_ROW_HEIGHT);

    font_flex.end();
    font_group.resizable(&font_flex);
    font_group.end();

    let mut result_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    result_group.set_label("Result View");
    result_group.set_color(theme::panel_bg());
    result_group.set_label_color(theme::text_secondary());
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
    result_cell_max_row.fixed(&result_cell_max_label, label_width);
    let mut result_cell_max_input = IntInput::default();
    result_cell_max_input.set_value(&config.result_cell_max_chars.to_string());
    result_cell_max_input.set_color(theme::input_bg());
    result_cell_max_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut result_cell_max_input);
    result_cell_max_row.end();
    result_flex.fixed(&result_cell_max_row, INPUT_ROW_HEIGHT);

    let mut lazy_fetch_batch_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    lazy_fetch_batch_row.set_type(FlexType::Row);
    lazy_fetch_batch_row.set_spacing(DIALOG_SPACING);
    let mut lazy_fetch_batch_label = Frame::default().with_label("Lazy Fetch:");
    lazy_fetch_batch_label.set_label_color(theme::text_primary());
    lazy_fetch_batch_row.fixed(&lazy_fetch_batch_label, label_width);
    let mut lazy_fetch_batch_input = IntInput::default();
    lazy_fetch_batch_input.set_value(&config.normalized_lazy_fetch_batch_size().to_string());
    lazy_fetch_batch_input.set_color(theme::input_bg());
    lazy_fetch_batch_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut lazy_fetch_batch_input);
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

    let mut intellisense_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    intellisense_group.set_label("IntelliSense");
    intellisense_group.set_color(theme::panel_bg());
    intellisense_group.set_label_color(theme::text_secondary());
    intellisense_group.begin();

    let mut intellisense_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    intellisense_flex.set_type(FlexType::Column);
    intellisense_flex.set_spacing(DIALOG_SPACING);

    let mut context_window_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    context_window_row.set_type(FlexType::Row);
    context_window_row.set_spacing(DIALOG_SPACING);
    let mut context_window_label = Frame::default().with_label("Context Window:");
    context_window_label.set_label_color(theme::text_primary());
    context_window_row.fixed(&context_window_label, label_width);
    let mut context_window_input = IntInput::default();
    context_window_input.set_value(
        &config
            .normalized_intellisense_context_window_kib()
            .to_string(),
    );
    context_window_input.set_color(theme::input_bg());
    context_window_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut context_window_input);
    context_window_row.end();
    intellisense_flex.fixed(&context_window_row, INPUT_ROW_HEIGHT);

    let mut popup_delay_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    popup_delay_row.set_type(FlexType::Row);
    popup_delay_row.set_spacing(DIALOG_SPACING);
    let mut popup_delay_label = Frame::default().with_label("Popup Delay:");
    popup_delay_label.set_label_color(theme::text_primary());
    popup_delay_row.fixed(&popup_delay_label, label_width);
    let mut popup_delay_input = IntInput::default();
    popup_delay_input.set_value(&config.normalized_intellisense_popup_delay_ms().to_string());
    popup_delay_input.set_color(theme::input_bg());
    popup_delay_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut popup_delay_input);
    popup_delay_row.end();
    intellisense_flex.fixed(&popup_delay_row, INPUT_ROW_HEIGHT);

    let mut context_window_hint = Frame::default().with_label(&format!(
        "Cursor context: {} ~ {} KiB (larger values use more CPU)",
        MIN_INTELLISENSE_CONTEXT_WINDOW_KIB, MAX_INTELLISENSE_CONTEXT_WINDOW_KIB
    ));
    context_window_hint.set_label_color(theme::text_secondary());
    intellisense_flex.fixed(&context_window_hint, LABEL_ROW_HEIGHT);

    let mut popup_delay_hint = Frame::default().with_label(&format!(
        "Popup delay: {} ~ {} ms",
        MIN_INTELLISENSE_POPUP_DELAY_MS, MAX_INTELLISENSE_POPUP_DELAY_MS
    ));
    popup_delay_hint.set_label_color(theme::text_secondary());
    intellisense_flex.fixed(&popup_delay_hint, LABEL_ROW_HEIGHT);

    let intellisense_filler = Frame::default();
    intellisense_flex.resizable(&intellisense_filler);
    intellisense_flex.end();
    intellisense_group.resizable(&intellisense_flex);
    intellisense_group.end();

    let mut connection_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    connection_group.set_label("Connection");
    connection_group.set_color(theme::panel_bg());
    connection_group.set_label_color(theme::text_secondary());
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
    pool_size_row.fixed(&pool_size_label, label_width);
    let mut pool_size_input = IntInput::default();
    pool_size_input.set_value(&config.normalized_connection_pool_size().to_string());
    pool_size_input.set_color(theme::input_bg());
    pool_size_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut pool_size_input);
    pool_size_row.end();
    connection_flex.fixed(&pool_size_row, INPUT_ROW_HEIGHT);

    let mut connect_timeout_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    connect_timeout_row.set_type(FlexType::Row);
    connect_timeout_row.set_spacing(DIALOG_SPACING);
    let mut connect_timeout_label = Frame::default().with_label("Connect Timeout:");
    connect_timeout_label.set_label_color(theme::text_primary());
    connect_timeout_row.fixed(&connect_timeout_label, label_width);
    let mut connect_timeout_input = IntInput::default();
    connect_timeout_input.set_value(&config.normalized_connect_timeout_seconds().to_string());
    connect_timeout_input.set_color(theme::input_bg());
    connect_timeout_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut connect_timeout_input);
    connect_timeout_row.end();
    connection_flex.fixed(&connect_timeout_row, INPUT_ROW_HEIGHT);

    let mut cancel_timeout_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    cancel_timeout_row.set_type(FlexType::Row);
    cancel_timeout_row.set_spacing(DIALOG_SPACING);
    let mut cancel_timeout_label = Frame::default().with_label("Cancel Timeout:");
    cancel_timeout_label.set_label_color(theme::text_primary());
    cancel_timeout_row.fixed(&cancel_timeout_label, label_width);
    let mut cancel_timeout_input = IntInput::default();
    cancel_timeout_input.set_value(&config.normalized_cancel_timeout_seconds().to_string());
    cancel_timeout_input.set_color(theme::input_bg());
    cancel_timeout_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut cancel_timeout_input);
    cancel_timeout_row.end();
    connection_flex.fixed(&cancel_timeout_row, INPUT_ROW_HEIGHT);

    let mut pool_hint = Frame::default().with_label(&format!(
        "Pool size: {} ~ {} (next connect)",
        MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE
    ));
    pool_hint.set_label_color(theme::text_secondary());
    connection_flex.fixed(&pool_hint, LABEL_ROW_HEIGHT);

    let mut connect_timeout_hint = Frame::default().with_label(&format!(
        "Connect timeout: {} ~ {} sec",
        MIN_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS
    ));
    connect_timeout_hint.set_label_color(theme::text_secondary());
    connection_flex.fixed(&connect_timeout_hint, LABEL_ROW_HEIGHT);

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
    tabs.insert(&connection_group, 0);

    let mut formatting_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    formatting_group.set_label("SQL Formatting");
    formatting_group.set_color(theme::panel_bg());
    formatting_group.set_label_color(theme::text_secondary());
    formatting_group.begin();

    let mut formatting_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    formatting_flex.set_type(FlexType::Column);
    formatting_flex.set_spacing(DIALOG_SPACING);

    let mut comma_layout_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    comma_layout_row.set_type(FlexType::Row);
    comma_layout_row.set_spacing(DIALOG_SPACING);
    let mut comma_layout_label = Frame::default().with_label("Comma Lists:");
    comma_layout_label.set_label_color(theme::text_primary());
    comma_layout_row.fixed(&comma_layout_label, label_width);
    let mut comma_layout_choice = Choice::default();
    comma_layout_choice.add_choice("Stacked|Wrapped");
    comma_layout_choice.set_value(match config.sql_comma_list_layout {
        SqlCommaListLayout::Stacked => 0,
        SqlCommaListLayout::Wrapped => 1,
    });
    theme::style_choice(&mut comma_layout_choice);
    theme::install_choice_hover(&mut comma_layout_choice);
    comma_layout_row.end();
    formatting_flex.fixed(&comma_layout_row, INPUT_ROW_HEIGHT);

    let mut right_margin_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    right_margin_row.set_type(FlexType::Row);
    right_margin_row.set_spacing(DIALOG_SPACING);
    let mut right_margin_label = Frame::default().with_label("Right Margin:");
    right_margin_label.set_label_color(theme::text_primary());
    right_margin_row.fixed(&right_margin_label, label_width);
    let mut right_margin_input = IntInput::default();
    right_margin_input.set_value(&config.normalized_sql_format_right_margin().to_string());
    right_margin_input.set_color(theme::input_bg());
    right_margin_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut right_margin_input);
    if config.sql_comma_list_layout == SqlCommaListLayout::Stacked {
        right_margin_input.deactivate();
    }
    right_margin_row.end();
    formatting_flex.fixed(&right_margin_row, INPUT_ROW_HEIGHT);

    let mut formatting_hint = Frame::default().with_label(&format!(
        "Wrapped margin: {} ~ {} columns",
        MIN_SQL_FORMAT_RIGHT_MARGIN, MAX_SQL_FORMAT_RIGHT_MARGIN
    ));
    formatting_hint.set_label_color(theme::text_secondary());
    formatting_flex.fixed(&formatting_hint, LABEL_ROW_HEIGHT);

    let formatting_filler = Frame::default();
    formatting_flex.resizable(&formatting_filler);
    formatting_flex.end();
    formatting_group.resizable(&formatting_flex);
    formatting_group.end();

    // These two labels are longer than the shared form label width.
    const RETENTION_LABEL_WIDTH: i32 = FORM_LABEL_WIDTH + 60;

    let mut history_group = Group::new(content_x, tab_body_y, content_w, tab_body_h, None);
    history_group.set_label("History && Log");
    history_group.set_color(theme::panel_bg());
    history_group.set_label_color(theme::text_secondary());
    history_group.begin();

    let mut history_flex = Flex::new(
        content_x + DIALOG_MARGIN,
        tab_body_y + DIALOG_MARGIN,
        content_w - DIALOG_MARGIN * 2,
        tab_body_h - DIALOG_MARGIN * 2,
        None,
    );
    history_flex.set_type(FlexType::Column);
    history_flex.set_spacing(DIALOG_SPACING);

    let mut history_limit_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    history_limit_row.set_type(FlexType::Row);
    history_limit_row.set_spacing(DIALOG_SPACING);
    let mut history_limit_label = Frame::default().with_label("Query History:");
    history_limit_label.set_label_color(theme::text_primary());
    history_limit_row.fixed(&history_limit_label, RETENTION_LABEL_WIDTH);
    let mut history_limit_input = IntInput::default();
    history_limit_input.set_value(&config.normalized_query_history_limit().to_string());
    history_limit_input.set_color(theme::input_bg());
    history_limit_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut history_limit_input);
    history_limit_input.set_tooltip("Number of executed queries kept in the history file");
    history_limit_row.end();
    history_flex.fixed(&history_limit_row, INPUT_ROW_HEIGHT);

    let mut app_log_limit_row = Flex::default().with_size(0, INPUT_ROW_HEIGHT);
    app_log_limit_row.set_type(FlexType::Row);
    app_log_limit_row.set_spacing(DIALOG_SPACING);
    let mut app_log_limit_label = Frame::default().with_label("Application Log:");
    app_log_limit_label.set_label_color(theme::text_primary());
    app_log_limit_row.fixed(&app_log_limit_label, RETENTION_LABEL_WIDTH);
    let mut app_log_limit_input = IntInput::default();
    app_log_limit_input.set_value(&config.normalized_app_log_limit().to_string());
    app_log_limit_input.set_color(theme::input_bg());
    app_log_limit_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut app_log_limit_input);
    app_log_limit_input.set_tooltip("Number of entries kept in the application log file");
    app_log_limit_row.end();
    history_flex.fixed(&app_log_limit_row, INPUT_ROW_HEIGHT);

    let mut history_limit_hint = Frame::default().with_label(&format!(
        "Query history: {} ~ {} entries, application log: {} ~ {} entries",
        MIN_QUERY_HISTORY_LIMIT, MAX_QUERY_HISTORY_LIMIT, MIN_APP_LOG_LIMIT, MAX_APP_LOG_LIMIT
    ));
    history_limit_hint.set_label_color(theme::text_secondary());
    history_flex.fixed(&history_limit_hint, LABEL_ROW_HEIGHT);

    let mut history_file_hint =
        Frame::default().with_label("Both are stored on disk and restored at the next start");
    history_file_hint.set_label_color(theme::text_secondary());
    history_flex.fixed(&history_file_hint, LABEL_ROW_HEIGHT);

    let history_filler = Frame::default();
    history_flex.resizable(&history_filler);
    history_flex.end();
    history_group.resizable(&history_flex);
    history_group.end();

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
    cancel_btn.set_color(theme::button_dark());
    cancel_btn.set_label_color(theme::text_primary());
    cancel_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut cancel_btn);
    let mut ok_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Save");
    ok_btn.set_color(theme::selection_soft());
    ok_btn.set_label_color(theme::text_primary());
    ok_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut ok_btn);
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

    let mut right_margin_input_for_layout = right_margin_input.clone();
    comma_layout_choice.set_callback(move |choice| {
        if choice.value() == 1 {
            right_margin_input_for_layout.activate();
        } else {
            right_margin_input_for_layout.deactivate();
        }
    });

    let result = Arc::new(Mutex::new(None::<FontSettings>));
    let result_for_ok = result.clone();
    let mut dialog_handle = dialog.clone();
    let editor_size_input_ok = editor_size_input.clone();
    let result_size_input_ok = result_size_input.clone();
    let global_size_input_ok = global_size_input.clone();
    let ui_scale_input_ok = ui_scale_input.clone();
    let result_cell_max_input_ok = result_cell_max_input.clone();
    let lazy_fetch_batch_input_ok = lazy_fetch_batch_input.clone();
    let context_window_input_ok = context_window_input.clone();
    let popup_delay_input_ok = popup_delay_input.clone();
    let pool_size_input_ok = pool_size_input.clone();
    let connect_timeout_input_ok = connect_timeout_input.clone();
    let cancel_timeout_input_ok = cancel_timeout_input.clone();
    let comma_layout_choice_ok = comma_layout_choice.clone();
    let right_margin_input_ok = right_margin_input.clone();
    let history_limit_input_ok = history_limit_input.clone();
    let app_log_limit_input_ok = app_log_limit_input.clone();
    let selected_font_ok = selected_font.clone();
    ok_btn.set_callback(move |_| {
        let ui_size = match validate_ui_size(&global_size_input_ok.value()) {
            Some(size) => size,
            None => return,
        };
        let ui_scale_percent = match validate_ui_scale_percent(&ui_scale_input_ok.value()) {
            Some(percent) => percent,
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
        let intellisense_context_window_kib =
            match validate_intellisense_context_window_kib(&context_window_input_ok.value()) {
                Some(size) => size,
                None => return,
            };
        let intellisense_popup_delay_ms =
            match validate_intellisense_popup_delay_ms(&popup_delay_input_ok.value()) {
                Some(delay_ms) => delay_ms,
                None => return,
            };
        let connection_pool_size = match validate_connection_pool_size(&pool_size_input_ok.value())
        {
            Some(size) => size,
            None => return,
        };
        let connect_timeout_seconds =
            match validate_connect_timeout_seconds(&connect_timeout_input_ok.value()) {
                Some(seconds) => seconds,
                None => return,
            };
        let cancel_timeout_seconds =
            match validate_cancel_timeout_seconds(&cancel_timeout_input_ok.value()) {
                Some(seconds) => seconds,
                None => return,
            };
        let sql_comma_list_layout = if comma_layout_choice_ok.value() == 1 {
            SqlCommaListLayout::Wrapped
        } else {
            SqlCommaListLayout::Stacked
        };
        let sql_format_right_margin =
            match validate_sql_format_right_margin(&right_margin_input_ok.value()) {
                Some(margin) => margin,
                None => return,
            };
        let query_history_limit =
            match validate_query_history_limit(&history_limit_input_ok.value()) {
                Some(limit) => limit,
                None => return,
            };
        let app_log_limit = match validate_app_log_limit(&app_log_limit_input_ok.value()) {
            Some(limit) => limit,
            None => return,
        };
        let font = selected_font_ok
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .trim()
            .to_string();
        if font.is_empty() {
            crate::ui::alert_on_main("Please select a font.");
            return;
        }
        *result_for_ok
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(FontSettings {
            font,
            ui_size,
            ui_scale_percent,
            editor_size,
            result_size,
            result_cell_max_chars,
            lazy_fetch_batch_size,
            intellisense_context_window_kib,
            intellisense_popup_delay_ms,
            connection_pool_size,
            connect_timeout_seconds,
            cancel_timeout_seconds,
            sql_comma_list_layout,
            sql_format_right_margin,
            query_history_limit,
            app_log_limit,
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
