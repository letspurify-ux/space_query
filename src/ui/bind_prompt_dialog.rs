//! The modal that asks for the placeholder values a statement needs before it
//! can run.
//!
//! Kept apart from [`crate::ui::bind_prompt`] so the scanning and substitution
//! rules stay free of FLTK and unit-testable.

use fltk::{
    app,
    button::{Button, CheckButton},
    enums::{Align, CallbackTrigger, FrameType},
    frame::Frame,
    group::{Flex, FlexType, Scroll, ScrollType},
    input::Input,
    menu::Choice,
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

use crate::ui::bind_prompt::{BindParam, BindParamType};
use crate::ui::constants::*;
use crate::ui::{center_on_main, theme};

/// Width of the placeholder-name column.
const NAME_WIDTH: i32 = 130;
/// Width of the type selector.
const TYPE_WIDTH: i32 = 110;
/// Width of the `NULL` checkbox column.
const NULL_WIDTH: i32 = 64;
/// Rows shown before the list starts scrolling.
const VISIBLE_ROWS: i32 = 8;
const ROW_HEIGHT: i32 = INPUT_ROW_HEIGHT;
/// Vertical gap between two rows.
const ROW_GAP: i32 = 4;
/// Height of the hint under the list. Two rows, because the sentence does not
/// fit on one at this width and a wrapped label needs the room to draw.
const HINT_HEIGHT: i32 = LABEL_ROW_HEIGHT * 2;

/// The widgets of one parameter row, read back when the user accepts.
struct ParamRow {
    type_choice: Choice,
    value_input: Input,
    null_check: CheckButton,
}

/// Ask for every value in `params`, which arrives prefilled with whatever the
/// tab remembered from the previous run. `types` is what the type selector
/// offers, which the caller narrows per backend. Returns `None` when cancelled,
/// which means the statement must not run at all.
pub fn show(params: &[BindParam], types: &[BindParamType]) -> Option<Vec<BindParam>> {
    if params.is_empty() {
        return Some(Vec::new());
    }

    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let width = 560;
    let row_count = i32::try_from(params.len()).unwrap_or(VISIBLE_ROWS);
    let list_height = row_count.min(VISIBLE_ROWS) * (ROW_HEIGHT + ROW_GAP) + ROW_GAP;
    let height = DIALOG_MARGIN * 2
        + list_height
        + DIALOG_SPACING
        + HINT_HEIGHT
        + DIALOG_SPACING
        + BUTTON_ROW_HEIGHT;

    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Bind Parameters");
    center_on_main(&mut dialog);
    dialog.set_color(theme::panel_raised());
    dialog.make_modal(true);

    let mut form = Flex::new(
        DIALOG_MARGIN,
        DIALOG_MARGIN,
        width - DIALOG_MARGIN * 2,
        height - DIALOG_MARGIN * 2,
        None,
    );
    form.set_type(FlexType::Column);
    form.set_spacing(DIALOG_SPACING);

    let mut scroll = Scroll::default();
    scroll.set_type(ScrollType::Vertical);
    scroll.set_color(theme::panel_bg());
    scroll.set_frame(FrameType::FlatBox);
    scroll.end();

    let mut hint = Frame::default().with_label(
        "Date and Timestamp values use YYYY-MM-DD HH:MM:SS. \
         Answers are kept and offered again on the next run.",
    );
    hint.set_label_color(theme::text_muted());
    hint.set_align(Align::Inside | Align::Left | Align::Wrap);
    form.fixed(&hint, HINT_HEIGHT);

    let mut button_row = Flex::default().with_size(0, BUTTON_ROW_HEIGHT);
    button_row.set_type(FlexType::Row);
    button_row.set_spacing(DIALOG_SPACING);
    let spacer = Frame::default();
    button_row.resizable(&spacer);
    let mut cancel_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Cancel");
    cancel_btn.set_color(theme::button_dark());
    cancel_btn.set_label_color(theme::text_primary());
    cancel_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut cancel_btn);
    let mut run_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Run");
    run_btn.set_color(theme::selection_soft());
    run_btn.set_label_color(theme::text_primary());
    run_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut run_btn);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.fixed(&run_btn, BUTTON_WIDTH);
    button_row.end();
    form.fixed(&button_row, BUTTON_ROW_HEIGHT);

    form.end();
    dialog.end();
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    // The Scroll's own width is not final until the first draw, so rows are
    // sized from the dialog the same way the import dialog sizes its mapping
    // rows.
    let usable = width - DIALOG_MARGIN * 2 - DIALOG_SPACING - app::scrollbar_size();
    let rows = build_rows(&mut scroll, params, types, usable);

    let result: Arc<Mutex<Option<Vec<BindParam>>>> = Arc::new(Mutex::new(None));

    {
        let result = result.clone();
        let mut dialog = dialog.clone();
        let params = params.to_vec();
        let types = types.to_vec();
        let rows = rows.clone();
        let mut accept = move || {
            let rows = rows.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let answered = params
                .iter()
                .zip(rows.iter())
                .map(|(param, row)| BindParam {
                    param_type: types
                        .get(usize::try_from(row.type_choice.value().max(0)).unwrap_or_default())
                        .copied()
                        .unwrap_or_default(),
                    value: row.value_input.value(),
                    is_null: row.null_check.is_checked(),
                    ..param.clone()
                })
                .collect();
            *result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(answered);
            dialog.hide();
            app::awake();
        };
        run_btn.set_callback(move |_| accept());
    }

    {
        let mut dialog = dialog.clone();
        cancel_btn.set_callback(move |_| {
            dialog.hide();
            app::awake();
        });
    }

    // Enter anywhere in the value column runs, so the common case — accept the
    // values already on screen — is one keystroke.
    for row in rows
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter_mut()
    {
        row.value_input.set_trigger(CallbackTrigger::EnterKeyAlways);
        let mut run_btn = run_btn.clone();
        row.value_input.set_callback(move |_| run_btn.do_callback());
    }

    if let Some(row) = rows
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .first_mut()
    {
        let _ = row.value_input.take_focus();
    }

    while dialog.shown() {
        app::wait();
    }

    Window::delete(dialog);

    // Bound rather than returned directly: the guard is a temporary whose drop
    // would otherwise outlive `result`.
    let answered = result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    answered
}

/// The type a row's selector currently names.
fn selected_type(choice: &Choice, types: &[BindParamType]) -> BindParamType {
    types
        .get(usize::try_from(choice.value().max(0)).unwrap_or_default())
        .copied()
        .unwrap_or_default()
}

/// A value and a `NULL` box only mean something for a type that takes a value.
///
/// `Ref Cursor` names a PL/SQL OUT parameter, so both controls go dead on that
/// row rather than inviting an answer the call cannot use.
fn sync_row_enablement(
    param_type: BindParamType,
    value_input: &mut Input,
    null_check: &mut CheckButton,
) {
    if !param_type.takes_a_value() {
        value_input.deactivate();
        null_check.deactivate();
        return;
    }
    null_check.activate();
    if null_check.is_checked() {
        value_input.deactivate();
    } else {
        value_input.activate();
    }
}

/// Lay one row out per parameter inside `scroll`, prefilled from `params`.
fn build_rows(
    scroll: &mut Scroll,
    params: &[BindParam],
    types: &[BindParamType],
    usable: i32,
) -> Arc<Mutex<Vec<ParamRow>>> {
    scroll.clear();
    let x = scroll.x() + DIALOG_SPACING;
    let mut y = scroll.y() + ROW_GAP;
    let value_width = (usable - NAME_WIDTH - TYPE_WIDTH - NULL_WIDTH - DIALOG_SPACING * 4).max(120);

    let mut built = Vec::with_capacity(params.len());
    scroll.begin();
    for param in params {
        let mut name = Frame::new(x, y, NAME_WIDTH, ROW_HEIGHT, None);
        name.set_label(&param.label);
        name.set_label_color(theme::text_primary());
        name.set_align(Align::Inside | Align::Left);

        let mut type_choice = Choice::new(
            x + NAME_WIDTH + DIALOG_SPACING,
            y,
            TYPE_WIDTH,
            ROW_HEIGHT,
            None,
        );
        for param_type in types {
            // The labels carry none of the characters FLTK parses in a menu
            // label, so they go in as written.
            type_choice.add_choice(param_type.label());
        }
        type_choice.set_value(
            types
                .iter()
                .position(|candidate| *candidate == param.param_type)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or_default(),
        );
        theme::style_choice(&mut type_choice);
        theme::install_choice_hover(&mut type_choice);

        let mut value_input = Input::new(
            x + NAME_WIDTH + TYPE_WIDTH + DIALOG_SPACING * 2,
            y,
            value_width,
            ROW_HEIGHT,
            None,
        );
        value_input.set_value(&param.value);
        value_input.set_color(theme::input_bg());
        value_input.set_text_color(theme::text_primary());
        theme::apply_text_input_inset(&mut value_input);

        let mut null_check = CheckButton::new(
            x + NAME_WIDTH + TYPE_WIDTH + value_width + DIALOG_SPACING * 3,
            y,
            NULL_WIDTH,
            ROW_HEIGHT,
            None,
        );
        null_check.set_label("NULL");
        null_check.set_label_color(theme::text_primary());
        null_check.set_selection_color(theme::accent());
        null_check.set_value(param.is_null);
        sync_row_enablement(param.param_type, &mut value_input, &mut null_check);

        {
            let mut value_input = value_input.clone();
            let type_choice = type_choice.clone();
            let types = types.to_vec();
            null_check.set_callback(move |check| {
                let mut check = check.clone();
                sync_row_enablement(
                    selected_type(&type_choice, &types),
                    &mut value_input,
                    &mut check,
                );
            });
        }
        {
            let mut value_input = value_input.clone();
            let mut null_check = null_check.clone();
            let types = types.to_vec();
            type_choice.set_callback(move |choice| {
                sync_row_enablement(
                    selected_type(choice, &types),
                    &mut value_input,
                    &mut null_check,
                );
            });
        }

        built.push(ParamRow {
            type_choice,
            value_input,
            null_check,
        });
        y += ROW_HEIGHT + ROW_GAP;
    }
    scroll.end();

    Arc::new(Mutex::new(built))
}
