//! The modal that hides and reorders a result grid's columns.
//!
//! Kept apart from [`crate::ui::column_layout`] so the rules about what a legal
//! arrangement is stay free of FLTK and unit-testable; this file only turns
//! clicks into calls on the plan.
//!
//! A list with buttons rather than draggable headers: the grid's column headers
//! already carry width dragging, drag-selection and the sort click, and adding a
//! fourth meaning to the same gesture makes all four harder to hit and none of
//! them verifiable in a GUI harness.

use fltk::{
    app,
    browser::HoldBrowser,
    button::Button,
    enums::FrameType,
    group::{Flex, FlexType},
    prelude::*,
    window::Window,
};
use std::sync::{Arc, Mutex};

use crate::ui::column_layout::ColumnLayoutPlan;
use crate::ui::constants::*;
use crate::ui::{alert_on_main, center_on_main, theme};

/// Marks in front of each column name. Two characters wide either way so the
/// names stay aligned.
const SHOWN_MARK: &str = "[x]";
const HIDDEN_MARK: &str = "[ ]";

/// Ask the user how the grid's columns should be arranged.
///
/// Returns the new arrangement, or `None` when nothing was changed or the
/// dialog was cancelled — so the caller never applies a no-op permutation.
pub(crate) fn show(initial: ColumnLayoutPlan) -> Option<ColumnLayoutPlan> {
    if initial.rows().is_empty() {
        return None;
    }

    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let width = 380;
    let list_height = 260;
    let height = DIALOG_MARGIN * 2 + list_height + DIALOG_SPACING * 2 + BUTTON_ROW_HEIGHT * 2;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Columns");
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

    let mut list = HoldBrowser::default();
    list.set_color(theme::input_bg());
    list.set_selection_color(theme::selection_strong());
    list.set_frame(FrameType::RFlatBox);
    list.set_tooltip("Double-click a column to show or hide it");

    let mut tool_row = Flex::default().with_size(0, BUTTON_ROW_HEIGHT);
    tool_row.set_type(FlexType::Row);
    tool_row.set_spacing(DIALOG_SPACING);
    let mut toggle_btn = tool_button("Show / Hide");
    let mut up_btn = tool_button("Move Up");
    let mut down_btn = tool_button("Move Down");
    let mut reset_btn = tool_button("Reset");
    tool_row.end();
    form.fixed(&tool_row, BUTTON_ROW_HEIGHT);

    let mut button_row = Flex::default().with_size(0, BUTTON_ROW_HEIGHT);
    button_row.set_type(FlexType::Row);
    button_row.set_spacing(DIALOG_SPACING);
    let spacer = fltk::frame::Frame::default();
    button_row.resizable(&spacer);
    let mut cancel_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Cancel");
    cancel_btn.set_color(theme::button_dark());
    cancel_btn.set_label_color(theme::text_primary());
    cancel_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut cancel_btn);
    let mut apply_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Apply");
    apply_btn.set_color(theme::selection_soft());
    apply_btn.set_label_color(theme::text_primary());
    apply_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut apply_btn);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.fixed(&apply_btn, BUTTON_WIDTH);
    button_row.end();
    form.fixed(&button_row, BUTTON_ROW_HEIGHT);

    form.resizable(&list);
    form.end();
    dialog.end();
    dialog.resizable(&form);
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    let plan = Arc::new(Mutex::new(initial));
    repaint(&mut list, &plan, 1);

    {
        let plan = plan.clone();
        let mut list = list.clone();
        toggle_btn.set_callback(move |_| toggle_selected(&mut list, &plan));
    }
    {
        let plan = plan.clone();
        let mut list = list.clone();
        up_btn.set_callback(move |_| move_selected(&mut list, &plan, false));
    }
    {
        let plan = plan.clone();
        let mut list = list.clone();
        down_btn.set_callback(move |_| move_selected(&mut list, &plan, true));
    }
    {
        let plan = plan.clone();
        let mut list = list.clone();
        reset_btn.set_callback(move |_| {
            plan.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .reset();
            repaint(&mut list, &plan, 1);
        });
    }
    {
        // A double-click in the list means the same thing as Show / Hide, which
        // is where the eye already is.
        let plan = plan.clone();
        list.set_callback(move |list| {
            if app::event_clicks() {
                toggle_selected(list, &plan);
            }
        });
    }

    let accepted = Arc::new(Mutex::new(false));
    {
        let accepted = accepted.clone();
        let mut dialog = dialog.clone();
        apply_btn.set_callback(move |_| {
            *accepted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            dialog.hide();
            app::awake();
        });
    }
    {
        let mut dialog = dialog.clone();
        cancel_btn.set_callback(move |_| {
            dialog.hide();
            app::awake();
        });
    }

    while dialog.shown() {
        app::wait();
    }
    Window::delete(dialog);

    let accepted = *accepted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !accepted {
        return None;
    }
    let plan = plan
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Some(plan)
}

fn tool_button(label: &str) -> Button {
    let mut button = Button::default().with_label(label);
    button.set_color(theme::button_subtle());
    button.set_label_color(theme::text_primary());
    button.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut button);
    button
}

/// Redraw the list from the plan and keep `select` selected.
///
/// The whole list is rebuilt on every change rather than patched: the plan is
/// the only state, and a list that is regenerated from it cannot drift out of
/// step with it.
fn repaint(list: &mut HoldBrowser, plan: &Arc<Mutex<ColumnLayoutPlan>>, select: i32) {
    let lines: Vec<String> = plan
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .rows()
        .iter()
        .map(|row| {
            let mark = if row.visible { SHOWN_MARK } else { HIDDEN_MARK };
            format!("{mark} {}", row.name)
        })
        .collect();
    list.clear();
    for line in &lines {
        list.add(line);
    }
    let clamped = select.clamp(1, i32::try_from(lines.len()).unwrap_or(1).max(1));
    list.select(clamped);
    list.redraw();
}

/// The selected row as an index into the plan, if anything is selected.
fn selected_index(list: &HoldBrowser) -> Option<usize> {
    let value = list.value();
    if value < 1 {
        return None;
    }
    usize::try_from(value - 1).ok()
}

fn toggle_selected(list: &mut HoldBrowser, plan: &Arc<Mutex<ColumnLayoutPlan>>) {
    let Some(index) = selected_index(list) else {
        return;
    };
    let outcome = {
        let mut plan = plan.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let visible = plan.rows().get(index).map(|row| row.visible);
        visible.map(|visible| plan.set_visible(index, !visible))
    };
    match outcome {
        Some(Err(message)) => alert_on_main(&message),
        Some(Ok(())) => {}
        None => return,
    }
    repaint(list, plan, i32::try_from(index).unwrap_or(0) + 1);
}

fn move_selected(list: &mut HoldBrowser, plan: &Arc<Mutex<ColumnLayoutPlan>>, forward: bool) {
    let Some(index) = selected_index(list) else {
        return;
    };
    let moved = plan
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .move_row(index, forward);
    let Some(target) = moved else {
        return;
    };
    // Selection follows the column, so repeated clicks keep moving the same one.
    repaint(list, plan, i32::try_from(target).unwrap_or(0) + 1);
}
