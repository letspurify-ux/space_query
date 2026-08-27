//! The modal that asks what an export should produce: format, rows, and where
//! the result goes.
//!
//! Kept apart from [`crate::ui::result_export`] so the serializers stay free of
//! FLTK and unit-testable.

use fltk::{
    app,
    button::{Button, RadioRoundButton},
    enums::FrameType,
    frame::Frame,
    group::{Flex, FlexType},
    menu::Choice,
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

use crate::ui::constants::*;
use crate::ui::result_export::{ExportDestination, ExportFormat, ExportScope};
use crate::ui::{center_on_main, theme};

/// What the export modal settled on.
///
/// Public with the module: the live harnesses render a tree export through the
/// production path and have to hand it the same value the modal returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportChoice {
    pub format: ExportFormat,
    pub scope: ExportScope,
    pub destination: ExportDestination,
}

/// Ask the user how to export. Returns `None` when the dialog is cancelled.
///
/// `formats` is what the caller can actually produce right now — `SQL Inserts`
/// drops out without a connection, because the literals it writes depend on the
/// dialect. `has_selection` false leaves the "Selected rows" option visible but
/// disabled, so the choice is discoverable before there is a selection to use it
/// on.
pub(crate) fn show(formats: &[ExportFormat], has_selection: bool) -> Option<ExportChoice> {
    if formats.is_empty() {
        return None;
    }

    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    // Wide enough that the longest radio label ("Selected rows") keeps clear of
    // the window edge at the default UI font size.
    let width = 440;
    let row_h = INPUT_ROW_HEIGHT;
    let height = DIALOG_MARGIN * 2 + row_h * 4 + DIALOG_SPACING * 3;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Export Results");
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

    let mut format_row = Flex::default().with_size(0, row_h);
    format_row.set_type(FlexType::Row);
    format_row.set_spacing(DIALOG_SPACING);
    let mut format_label = Frame::default().with_label("Format:");
    format_label.set_label_color(theme::text_primary());
    format_row.fixed(&format_label, FORM_LABEL_WIDTH);
    let mut format_choice = Choice::default();
    for format in formats {
        format_choice.add_choice(format.label());
    }
    format_choice.set_value(0);
    theme::style_choice(&mut format_choice);
    theme::install_choice_hover(&mut format_choice);
    format_row.end();
    form.fixed(&format_row, row_h);

    let (rows_row, all_rows, mut selected_rows) =
        radio_row("Rows:", "All rows", "Selected rows", row_h);
    if !has_selection {
        selected_rows.deactivate();
        selected_rows.set_tooltip("Select cells in the grid to export just those rows");
    }
    rows_row.end();
    form.fixed(&rows_row, row_h);

    let (destination_row, to_file, _to_clipboard) =
        radio_row("Destination:", "File", "Clipboard", row_h);
    destination_row.end();
    form.fixed(&destination_row, row_h);

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
    let mut export_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Export");
    export_btn.set_color(theme::selection_soft());
    export_btn.set_label_color(theme::text_primary());
    export_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut export_btn);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.fixed(&export_btn, BUTTON_WIDTH);
    button_row.end();
    form.fixed(&button_row, BUTTON_ROW_HEIGHT);

    form.end();
    dialog.end();
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    let result: Arc<Mutex<Option<ExportChoice>>> = Arc::new(Mutex::new(None));
    let formats = formats.to_vec();
    let result_for_export = result.clone();
    let mut dialog_for_export = dialog.clone();
    export_btn.set_callback(move |_| {
        let selected = usize::try_from(format_choice.value().max(0)).unwrap_or_default();
        let Some(format) = formats.get(selected).copied() else {
            return;
        };
        *result_for_export
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ExportChoice {
            format,
            scope: if all_rows.is_set() {
                ExportScope::All
            } else {
                ExportScope::Selection
            },
            destination: if to_file.is_set() {
                ExportDestination::File
            } else {
                ExportDestination::Clipboard
            },
        });
        dialog_for_export.hide();
        app::awake();
    });

    let mut dialog_for_cancel = dialog.clone();
    cancel_btn.set_callback(move |_| {
        dialog_for_cancel.hide();
        app::awake();
    });

    while dialog.shown() {
        app::wait();
    }

    // Explicitly destroy top-level dialog widgets to release native resources.
    Window::delete(dialog);

    let choice = result
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    choice
}

/// A labelled pair of radio buttons. They are siblings inside the returned row,
/// which is what makes FLTK treat them as one exclusive group.
fn radio_row(
    label: &str,
    first: &str,
    second: &str,
    row_h: i32,
) -> (Flex, RadioRoundButton, RadioRoundButton) {
    let mut row = Flex::default().with_size(0, row_h);
    row.set_type(FlexType::Row);
    row.set_spacing(DIALOG_SPACING);
    let mut row_label = Frame::default().with_label(label);
    row_label.set_label_color(theme::text_primary());
    row.fixed(&row_label, FORM_LABEL_WIDTH);

    let mut first_btn = RadioRoundButton::default().with_label(first);
    first_btn.set_label_color(theme::text_primary());
    first_btn.set_selection_color(theme::accent());
    first_btn.set_value(true);

    let mut second_btn = RadioRoundButton::default().with_label(second);
    second_btn.set_label_color(theme::text_primary());
    second_btn.set_selection_color(theme::accent());

    (row, first_btn, second_btn)
}
