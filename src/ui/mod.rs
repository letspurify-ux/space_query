pub(crate) mod builtin_signatures;
#[cfg(test)]
mod builtin_signatures_live_tests;
pub mod connection_dialog;
pub mod constants;
pub mod find_replace;
pub mod font_settings;
pub mod intellisense;
pub mod intellisense_context;
pub mod log_viewer;
#[cfg(target_os = "macos")]
pub(crate) mod macos_window_state;
pub mod main_window;
pub mod menu;
pub mod object_browser;
pub(crate) mod object_drag_payload;
pub mod query_history;
pub mod query_tabs;
pub mod result_table;
pub mod result_tabs;
pub mod settings_dialog;
pub(crate) mod sql_depth;
pub mod sql_editor;
pub mod syntax_highlight;
pub(crate) mod tab_strip;
pub mod table_browse;
pub(crate) mod text_buffer_access;
pub mod theme;
pub(crate) mod ui_timeout;

use fltk::{
    app,
    button::Button,
    enums::{Align, CallbackTrigger, Event, FrameType, Key},
    frame::Frame,
    group::{Flex, FlexType},
    input::Input,
    prelude::*,
    window::Window,
};

#[cfg(not(test))]
use fltk::enums::Font;

use crate::utils::arithmetic::safe_div;

pub use connection_dialog::*;
pub use find_replace::*;
pub use font_settings::*;
pub use intellisense::*;
pub use main_window::*;
pub use menu::*;
pub use object_browser::*;
pub use query_history::*;
pub use query_tabs::*;
pub use result_table::*;
pub use result_tabs::*;
pub use settings_dialog::*;
pub use sql_editor::*;
pub use syntax_highlight::*;
pub use table_browse::*;

#[derive(Clone)]
pub struct ResultTabRequest {
    pub label: String,
    pub result: crate::db::QueryResult,
}

pub fn center_on_main(window: &mut Window) {
    // NOTE: fltk-rs의 center_of()는 참조 위젯이 Window 타입이면
    // wx/wy를 0으로 고정해 실제 화면 위치를 무시하는 버그가 있음.
    // 메인 윈도우 좌표를 직접 읽어 set_pos()로 설정한다.
    let target = if let Some(main) = app::widget_from_id::<Window>("main_window") {
        if main.as_widget_ptr() != window.as_widget_ptr() {
            Some((main.x(), main.y(), main.width(), main.height()))
        } else {
            None
        }
    } else {
        app::first_window().map(|main| (main.x(), main.y(), main.width(), main.height()))
    };

    let (x, y) = if let Some((mx, my, mw, mh)) = target {
        (
            mx + safe_div(mw - window.width(), 2),
            my + safe_div(mh - window.height(), 2),
        )
    } else {
        let (sw, sh) = app::screen_size();
        (
            safe_div((sw as i32) - window.width(), 2),
            safe_div((sh as i32) - window.height(), 2),
        )
    };
    window.set_pos(x, y);
}

fn dialog_prompt_height(text: &str, available_width: i32) -> i32 {
    #[cfg(test)]
    let height = {
        let columns_per_line = safe_div(available_width - 16, 8).max(1);
        let line_count = text
            .lines()
            .map(|line| {
                let columns = line
                    .chars()
                    .map(|ch| if ch.is_ascii() { 1 } else { 2 })
                    .sum::<i32>()
                    .max(1);
                safe_div(columns + columns_per_line - 1, columns_per_line)
            })
            .sum::<i32>()
            .max(1);
        (line_count * 22 + 16).clamp(56, 420)
    };

    #[cfg(not(test))]
    let height = {
        let font_size = app::font_size().clamp(8, 24);
        fltk::draw::set_font(Font::Helvetica, font_size);
        let (_, measured_height) =
            fltk::draw::wrap_measure(text, (available_width - 16).max(1), false);
        measured_height.saturating_add(16).clamp(56, 420)
    };

    height
}

fn dialog_button_width(label: &str) -> i32 {
    #[cfg(test)]
    let width = (label.chars().count() as i32 * 8 + 28).clamp(constants::BUTTON_WIDTH, 320);

    #[cfg(not(test))]
    let width = {
        let font_size = app::font_size().clamp(8, 24);
        fltk::draw::set_font(Font::Helvetica, font_size);
        let text_width = fltk::draw::measure(label, false).0.saturating_add(28);
        text_width.clamp(constants::BUTTON_WIDTH, 320)
    };

    width
}

fn finish_modal_dialog(dialog: Window) {
    while dialog.shown() {
        app::wait();
    }

    Window::delete(dialog);
}

fn choice2_on_main_with_title(title: &str, txt: &str, b0: &str, b1: &str, b2: &str) -> Option<i32> {
    let choices = [(2, b2), (1, b1), (0, b0)]
        .into_iter()
        .filter(|(_, label)| !label.is_empty())
        .collect::<Vec<_>>();

    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let button_width = choices
        .iter()
        .map(|(_, label)| dialog_button_width(label))
        .sum::<i32>();
    let button_spacing = constants::DIALOG_SPACING
        .saturating_mul(i32::try_from(choices.len().saturating_sub(1)).unwrap_or(i32::MAX));
    let width = 520.max(
        constants::DIALOG_MARGIN
            .saturating_mul(2)
            .saturating_add(button_width)
            .saturating_add(button_spacing),
    );
    let content_width = width - constants::DIALOG_MARGIN * 2;
    let prompt_height = dialog_prompt_height(txt, content_width);
    let height = constants::DIALOG_MARGIN * 2
        + prompt_height
        + constants::DIALOG_SPACING
        + constants::BUTTON_ROW_HEIGHT;
    let mut dialog = Window::default().with_size(width, height).with_label(title);
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let mut main_flex = Flex::default()
        .with_pos(constants::DIALOG_MARGIN, constants::DIALOG_MARGIN)
        .with_size(
            width - constants::DIALOG_MARGIN * 2,
            height - constants::DIALOG_MARGIN * 2,
        );
    main_flex.set_type(FlexType::Column);
    main_flex.set_spacing(constants::DIALOG_SPACING);

    let mut prompt_frame = Frame::default().with_label(txt);
    prompt_frame.set_label_color(theme::text_primary());
    prompt_frame.set_align(Align::Left | Align::Inside | Align::Wrap);
    main_flex.fixed(&prompt_frame, prompt_height);

    let mut button_flex = Flex::default();
    button_flex.set_type(FlexType::Row);
    button_flex.set_spacing(constants::DIALOG_SPACING);

    let _spacer = Frame::default();

    let result = std::sync::Arc::new(std::sync::Mutex::new(None::<i32>));
    let mut buttons = Vec::new();
    for (choice_index, label) in choices {
        let mut button = Button::default()
            .with_size(dialog_button_width(label), constants::BUTTON_HEIGHT)
            .with_label(label);
        button.set_color(theme::button_dark());
        button.set_label_color(theme::text_primary());
        button.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut button);

        let result_for_button = result.clone();
        let mut dialog_for_button = dialog.clone();
        button.set_callback(move |_| {
            *result_for_button
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(choice_index);
            dialog_for_button.hide();
            app::awake();
        });

        let button_width = button.width();
        button_flex.fixed(&button, button_width);
        buttons.push((choice_index, button));
    }
    button_flex.end();
    main_flex.fixed(&button_flex, constants::BUTTON_ROW_HEIGHT);
    main_flex.end();
    dialog.end();
    fltk::group::Group::set_current(current_group.as_ref());

    {
        let result = result.clone();
        dialog.set_callback(move |window| {
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            window.hide();
            app::awake();
        });
    }

    {
        let result = result.clone();
        let default_choice = if buttons.iter().any(|(index, _)| *index == 1) {
            Some(1)
        } else if buttons.iter().any(|(index, _)| *index == 0) {
            Some(0)
        } else {
            buttons.first().map(|(index, _)| *index)
        };
        dialog.handle(move |window, event| match event {
            Event::KeyDown if matches!(app::event_key(), Key::Enter | Key::KPEnter) => {
                *result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = default_choice;
                window.hide();
                app::awake();
                true
            }
            Event::KeyDown if app::event_key() == Key::Escape => {
                *result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                window.hide();
                app::awake();
                true
            }
            _ => false,
        });
    }

    dialog.show();
    let focus_button_index = buttons
        .iter()
        .position(|(index, _)| *index == 1)
        .or_else(|| (!buttons.is_empty()).then_some(0));
    if let Some(index) = focus_button_index {
        buttons[index].1.take_focus().ok();
    }

    finish_modal_dialog(dialog);
    let choice = *result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    choice
}

pub fn alert_on_main(txt: &str) {
    let _ = choice2_on_main_with_title("Alert", txt, "Close", "", "");
}

pub fn message_on_main(txt: &str) {
    let _ = choice2_on_main_with_title("Message", txt, "Close", "", "");
}

pub fn choice2_on_main(txt: &str, b0: &str, b1: &str, b2: &str) -> Option<i32> {
    choice2_on_main_with_title("Question", txt, b0, b1, b2)
}

pub fn input_on_main(txt: &str, deflt: &str) -> Option<String> {
    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let width = 520;
    let content_width = width - constants::DIALOG_MARGIN * 2;
    let prompt_height = dialog_prompt_height(txt, content_width);
    let height = constants::DIALOG_MARGIN * 2
        + prompt_height
        + constants::DIALOG_SPACING
        + constants::INPUT_ROW_HEIGHT
        + constants::DIALOG_SPACING
        + constants::BUTTON_ROW_HEIGHT;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Input");
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let mut main_flex = Flex::default()
        .with_pos(constants::DIALOG_MARGIN, constants::DIALOG_MARGIN)
        .with_size(
            width - constants::DIALOG_MARGIN * 2,
            height - constants::DIALOG_MARGIN * 2,
        );
    main_flex.set_type(FlexType::Column);
    main_flex.set_spacing(constants::DIALOG_SPACING);

    let mut prompt_frame = Frame::default().with_label(txt);
    prompt_frame.set_label_color(theme::text_primary());
    prompt_frame.set_align(Align::Left | Align::Inside | Align::Wrap);
    main_flex.fixed(&prompt_frame, prompt_height);

    let mut input = Input::default();
    input.set_color(theme::input_bg());
    input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut input);
    input.set_value(deflt);
    input.set_trigger(CallbackTrigger::EnterKeyAlways);
    main_flex.fixed(&input, constants::INPUT_ROW_HEIGHT);

    let mut button_flex = Flex::default();
    button_flex.set_type(FlexType::Row);
    button_flex.set_spacing(constants::DIALOG_SPACING);

    let _spacer = Frame::default();

    let ok_button_width = dialog_button_width("OK");
    let cancel_button_width = dialog_button_width("Cancel");
    let mut ok_btn = Button::default()
        .with_size(ok_button_width, constants::BUTTON_HEIGHT)
        .with_label("OK");
    ok_btn.set_color(theme::button_dark());
    ok_btn.set_label_color(theme::text_primary());
    ok_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut ok_btn);

    let mut cancel_btn = Button::default()
        .with_size(cancel_button_width, constants::BUTTON_HEIGHT)
        .with_label("Cancel");
    cancel_btn.set_color(theme::button_dark());
    cancel_btn.set_label_color(theme::text_primary());
    cancel_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut cancel_btn);

    button_flex.fixed(&ok_btn, ok_button_width);
    button_flex.fixed(&cancel_btn, cancel_button_width);
    button_flex.end();
    main_flex.fixed(&button_flex, constants::BUTTON_ROW_HEIGHT);
    main_flex.end();
    dialog.end();
    fltk::group::Group::set_current(current_group.as_ref());

    let result = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));

    {
        let result = result.clone();
        let mut dialog = dialog.clone();
        let input = input.clone();
        ok_btn.set_callback(move |_| {
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(input.value());
            dialog.hide();
            app::awake();
        });
    }

    {
        let result = result.clone();
        let mut dialog = dialog.clone();
        let mut input_cb = input.clone();
        let input_value = input.clone();
        input_cb.set_callback(move |_| {
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(input_value.value());
            dialog.hide();
            app::awake();
        });
    }

    {
        let result = result.clone();
        let mut dialog = dialog.clone();
        cancel_btn.set_callback(move |_| {
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            dialog.hide();
            app::awake();
        });
    }

    {
        let result = result.clone();
        dialog.set_callback(move |window| {
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            window.hide();
            app::awake();
        });
    }

    {
        let result = result.clone();
        dialog.handle(move |window, event| match event {
            Event::KeyDown if app::event_key() == Key::Escape => {
                *result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                window.hide();
                app::awake();
                true
            }
            _ => false,
        });
    }

    dialog.show();
    input.take_focus().ok();

    finish_modal_dialog(dialog);
    let value = result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PopupAnchorSnapshot {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    shown: bool,
    visible: bool,
}

impl PopupAnchorSnapshot {
    pub(crate) fn capture<W: WidgetExt>(anchor: &W) -> Option<Self> {
        if anchor.was_deleted() {
            return None;
        }

        let window = anchor.top_window()?;
        Some(Self {
            x: window.x_root(),
            y: window.y_root(),
            w: window.w(),
            h: window.h(),
            shown: window.shown(),
            visible: window.visible(),
        })
    }

    pub(crate) fn still_matches<W: WidgetExt>(self, anchor: &W) -> bool {
        Self::capture(anchor)
            .filter(|current| current.shown && current.visible)
            .is_some_and(|current| current == self)
    }
}

#[cfg(test)]
mod tests {
    use super::dialog_prompt_height;

    #[test]
    fn dialog_prompt_height_keeps_short_text_compact() {
        assert_eq!(dialog_prompt_height("Short message", 500), 56);
    }

    #[test]
    fn dialog_prompt_height_grows_for_wrapped_long_text() {
        let short_height = dialog_prompt_height("Short message", 500);
        let long_message = "This alert message is intentionally long enough to wrap across several lines in the shared dialog window so the prompt area must grow instead of clipping the content.";

        assert!(dialog_prompt_height(long_message, 500) > short_height);
    }

    #[test]
    fn dialog_prompt_height_counts_non_ascii_text_wider() {
        let ascii = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let korean = "가가가가가가가가가가가가가가가가가가가가";

        assert!(dialog_prompt_height(korean, 120) >= dialog_prompt_height(ascii, 120));
    }
}
