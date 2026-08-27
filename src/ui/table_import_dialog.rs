//! The modal that asks how a file should be loaded into a table: which format
//! it is in, how it spells a header and NULL, and which file column feeds which
//! table column.
//!
//! Kept apart from [`crate::ui::result_import`] and
//! [`crate::ui::table_import`] so the parsers and the SQL builder stay free of
//! FLTK and unit-testable.
//!
//! The dialog re-reads the file whenever an option changes, so the column list
//! and the row count on screen always describe what pressing Import would
//! actually run.

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

use crate::ui::constants::*;
use crate::ui::result_export::ExportFormat;
use crate::ui::result_import::{
    header_choice_applies, null_text_choice_applies, parse, ImportOptions, ImportedTable,
};
use crate::ui::table_import::{
    check_mapping, default_mapping, ColumnMapping, ImportTargets, TargetColumn,
};
use crate::ui::widget_label::add_menu_item;
use crate::ui::{center_on_main, theme};

/// Height of one source-column row inside the mapping area — the same height
/// every other interactive control in the app uses.
const MAPPING_ROW_HEIGHT: i32 = INPUT_ROW_HEIGHT;
/// Width of the source-column name shown at the left of a mapping row.
const MAPPING_NAME_WIDTH: i32 = 220;

/// What the user settled on. `data` is the file as it parsed under the options
/// on screen, so the caller never has to parse it a second time and risk a
/// different answer.
pub(crate) struct ImportOutcome {
    pub mapping: ColumnMapping,
    pub data: ImportedTable,
}

/// Everything the callbacks share.
struct DialogState {
    text: String,
    /// Every column of the table, writable or not, in the table's own order.
    ///
    /// Held whole rather than pre-filtered because a headerless file is mapped
    /// by POSITION, and a position means a place in the TABLE — see
    /// [`ImportTargets::positional_mapping`].
    table_columns: ImportTargets,
    /// The subset a mapping points at, in table order. Derived once so the
    /// mapping indexes and the dropdown items cannot come from two readings.
    targets: Vec<TargetColumn>,
    /// Columns the table has that no value may be written into, so the summary
    /// can name them. They are deliberately absent from `targets`.
    generated_columns: Vec<String>,
    formats: Vec<ExportFormat>,
    /// The width a mapping row gets. Taken from the dialog's own size rather
    /// than from the Scroll, whose Flex-assigned width is not final until the
    /// first draw — which happens after the rows are built.
    row_width: i32,
    /// The file as it parsed under the current options, or why it did not.
    parsed: Mutex<Result<ImportedTable, String>>,
    /// One Choice per file column: 0 is "skip", n+1 is target column n.
    mapping_choices: Mutex<Vec<Choice>>,
    /// What the mapping area on screen was built for: the file's columns, or the
    /// parse error it is showing instead.
    ///
    /// Rebuilding that area is what discards the user's picks, so it happens
    /// only when this changes. It used to happen on every reload, and a reload
    /// runs on every keystroke in the NULL text — which does not change the
    /// file's columns at all, so a hand-made mapping was thrown away by typing.
    /// The error is part of the key because the area SHOWS it: two different
    /// errors are two different things to draw.
    mapped_columns: Mutex<Option<Result<Vec<String>, String>>>,
}

impl DialogState {
    fn options(&self, format: ExportFormat, header: bool, null_text: String) -> ImportOptions {
        ImportOptions {
            format,
            has_header: header,
            null_text,
        }
    }

    /// The mapping the Choice widgets currently express.
    fn mapping(&self) -> ColumnMapping {
        self.mapping_choices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|choice| {
                let value = choice.value();
                if value <= 0 {
                    None
                } else {
                    usize::try_from(value - 1).ok()
                }
            })
            .collect()
    }
}

/// Ask how to import `text` into `table_label`.
///
/// `initial_format` is what the file's extension suggested; the user can pick
/// another. Returns `None` when the dialog is cancelled.
pub(crate) fn show(
    file_label: &str,
    text: &str,
    table_label: &str,
    table_columns: &ImportTargets,
    initial_format: ExportFormat,
) -> Option<ImportOutcome> {
    let targets = table_columns.writable();
    let generated_columns = table_columns.generated_names();
    if targets.is_empty() {
        crate::ui::alert_on_main(if generated_columns.is_empty() {
            "The table has no columns to import into."
        } else {
            "Every column of this table is computed by the server, so there is \
             nothing an import could write."
        });
        return None;
    }

    let current_group = fltk::group::Group::try_current();
    fltk::group::Group::set_current(None::<&fltk::group::Group>);

    let width = 620;
    let height = 520;
    let row_h = INPUT_ROW_HEIGHT;
    let mut dialog = Window::default()
        .with_size(width, height)
        .with_label("Import Data from File");
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

    let file_row = read_only_row("File:", file_label, row_h);
    form.fixed(&file_row, row_h);
    let table_row = read_only_row("Table:", table_label, row_h);
    form.fixed(&table_row, row_h);

    let formats: Vec<ExportFormat> = ExportFormat::ALL.into_iter().collect();
    let mut format_row = Flex::default().with_size(0, row_h);
    format_row.set_type(FlexType::Row);
    format_row.set_spacing(DIALOG_SPACING);
    let mut format_label = Frame::default().with_label("Format:");
    format_label.set_label_color(theme::text_primary());
    format_row.fixed(&format_label, FORM_LABEL_WIDTH);
    let mut format_choice = Choice::default();
    for format in &formats {
        format_choice.add_choice(format.label());
    }
    format_choice.set_value(
        i32::try_from(
            formats
                .iter()
                .position(|format| *format == initial_format)
                .unwrap_or_default(),
        )
        .unwrap_or_default(),
    );
    theme::style_choice(&mut format_choice);
    theme::install_choice_hover(&mut format_choice);
    let mut header_check = CheckButton::default().with_label("First row is a header");
    header_check.set_value(true);
    header_check.set_label_color(theme::text_primary());
    header_check.set_selection_color(theme::accent());
    format_row.end();
    form.fixed(&format_row, row_h);

    let mut null_row = Flex::default().with_size(0, row_h);
    null_row.set_type(FlexType::Row);
    null_row.set_spacing(DIALOG_SPACING);
    let mut null_label = Frame::default().with_label("NULL text:");
    null_label.set_label_color(theme::text_primary());
    null_row.fixed(&null_label, FORM_LABEL_WIDTH);
    let mut null_input = Input::default();
    null_input.set_value(&ImportOptions::default().null_text);
    null_input.set_color(theme::input_bg());
    null_input.set_text_color(theme::text_primary());
    theme::apply_text_input_inset(&mut null_input);
    null_input.set_tooltip("A CSV or TSV cell holding exactly this text is imported as SQL NULL");
    let mut null_hint = Frame::default().with_label("(CSV and TSV only)");
    null_hint.set_label_color(theme::text_muted());
    null_hint.set_align(Align::Inside | Align::Left);
    null_row.end();
    form.fixed(&null_row, row_h);

    let mut columns_label = Frame::default().with_label("File column → table column");
    columns_label.set_label_color(theme::text_secondary());
    columns_label.set_align(Align::Inside | Align::Left);
    form.fixed(&columns_label, LABEL_ROW_HEIGHT);

    let mut scroll = Scroll::default();
    scroll.set_type(ScrollType::Vertical);
    scroll.set_color(theme::panel_bg());
    scroll.set_frame(FrameType::FlatBox);
    scroll.end();

    let mut summary = Frame::default();
    summary.set_label_color(theme::text_secondary());
    summary.set_align(Align::Inside | Align::Left | Align::Wrap);
    form.fixed(&summary, LABEL_ROW_HEIGHT * 2);

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
    let mut import_btn = Button::default()
        .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
        .with_label("Import");
    import_btn.set_color(theme::selection_soft());
    import_btn.set_label_color(theme::text_primary());
    import_btn.set_frame(FrameType::RFlatBox);
    theme::install_button_hover(&mut import_btn);
    button_row.fixed(&cancel_btn, BUTTON_WIDTH);
    button_row.fixed(&import_btn, BUTTON_WIDTH);
    button_row.end();
    form.fixed(&button_row, BUTTON_ROW_HEIGHT);

    form.end();
    dialog.end();
    dialog.show();
    fltk::group::Group::set_current(current_group.as_ref());

    let state = Arc::new(DialogState {
        text: text.to_string(),
        table_columns: table_columns.clone(),
        targets,
        generated_columns,
        formats,
        row_width: width - DIALOG_MARGIN * 2 - DIALOG_SPACING * 2 - app::scrollbar_size(),
        parsed: Mutex::new(Err(String::new())),
        mapping_choices: Mutex::new(Vec::new()),
        mapped_columns: Mutex::new(None),
    });

    let reload = {
        let state = Arc::clone(&state);
        let format_choice = format_choice.clone();
        let header_check = header_check.clone();
        let null_input = null_input.clone();
        let mut header_check_for_gating = header_check.clone();
        let mut null_input_for_gating = null_input.clone();
        let mut scroll = scroll.clone();
        let mut summary = summary.clone();
        move || {
            let format = state
                .formats
                .get(usize::try_from(format_choice.value().max(0)).unwrap_or_default())
                .copied()
                .unwrap_or(ExportFormat::Csv);
            // A choice that means nothing for this format must not look live.
            gate(&mut header_check_for_gating, header_choice_applies(format));
            gate(&mut null_input_for_gating, null_text_choice_applies(format));

            let options = state.options(format, header_check.is_set(), null_input.value());
            let parsed = parse(&state.text, &options);
            // The mapping rows describe the FILE's columns. When those are the
            // same as the ones on screen, the rows already on screen describe
            // them — including any target the user picked by hand.
            let columns = parsed
                .as_ref()
                .map(|data| data.columns.clone())
                .map_err(String::clone);
            let rebuild = {
                let mut built_for = state
                    .mapped_columns
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let changed = built_for.as_ref() != Some(&columns);
                if changed {
                    *built_for = Some(columns);
                }
                changed
            };
            if rebuild {
                rebuild_mapping_rows(&state, &mut scroll, &parsed, options.has_header);
            }
            *state
                .parsed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed;
            update_summary(&state, &mut summary);
        }
    };

    let refresh_summary = {
        let state = Arc::clone(&state);
        let mut summary = summary.clone();
        move || update_summary(&state, &mut summary)
    };
    *SUMMARY_REFRESH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(refresh_summary));

    {
        let mut reload_on_change = reload.clone();
        format_choice.set_callback(move |_| reload_on_change());
    }
    {
        let mut reload_on_change = reload.clone();
        header_check.set_callback(move |_| reload_on_change());
    }
    {
        let mut reload_on_change = reload.clone();
        null_input.set_trigger(CallbackTrigger::Changed);
        null_input.set_callback(move |_| reload_on_change());
    }
    {
        let mut reload_once = reload.clone();
        reload_once();
    }

    let outcome: Arc<Mutex<Option<ImportOutcome>>> = Arc::new(Mutex::new(None));
    {
        let state = Arc::clone(&state);
        let outcome = Arc::clone(&outcome);
        let mut dialog = dialog.clone();
        import_btn.set_callback(move |_| {
            // Cloned out so the `parsed` guard dies before the match: matching
            // on the guard itself would hold it across the Err arm's modal
            // alert, and a nested `app::wait` may not run under this lock.
            let parsed = state
                .parsed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let data = match parsed {
                Ok(data) => data,
                Err(error) => {
                    crate::ui::alert_on_main(&error);
                    return;
                }
            };
            let mapping = state.mapping();
            // The SAME validator the script builder runs, asked while the
            // dialog is still open: a mapping it refuses can be corrected here
            // instead of costing the user the file chooser and every choice
            // they made in this dialog.
            if let Err(error) = check_mapping(&state.targets, &mapping, &data) {
                crate::ui::alert_on_main(&error);
                return;
            }
            *outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(ImportOutcome { mapping, data });
            dialog.hide();
            app::awake();
        });
    }

    let mut dialog_for_cancel = dialog.clone();
    cancel_btn.set_callback(move |_| {
        dialog_for_cancel.hide();
        app::awake();
    });

    while dialog.shown() {
        app::wait();
    }

    *SUMMARY_REFRESH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    Window::delete(dialog);

    let outcome = outcome
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    outcome
}

/// A label and a value the user cannot edit.
fn read_only_row(label: &str, value: &str, row_h: i32) -> Flex {
    let mut row = Flex::default().with_size(0, row_h);
    row.set_type(FlexType::Row);
    row.set_spacing(DIALOG_SPACING);
    let mut row_label = Frame::default().with_label(label);
    row_label.set_label_color(theme::text_primary());
    row.fixed(&row_label, FORM_LABEL_WIDTH);
    let mut row_value = Frame::default().with_label(value);
    row_value.set_label_color(theme::text_secondary());
    row_value.set_align(Align::Inside | Align::Left);
    row.end();
    row
}

fn gate<W: WidgetExt>(widget: &mut W, active: bool) {
    if active {
        widget.activate();
    } else {
        widget.deactivate();
    }
}

/// Replace the mapping rows with one per file column.
///
/// With a header the file names its columns, so they are matched to the table
/// by name. Without one the names are positional placeholders, so the columns
/// are matched by position instead — which is the only thing a header-less file
/// can mean.
fn rebuild_mapping_rows(
    state: &DialogState,
    scroll: &mut Scroll,
    parsed: &Result<ImportedTable, String>,
    has_header: bool,
) {
    scroll.clear();
    let mut choices: Vec<Choice> = Vec::new();
    let x = scroll.x() + DIALOG_SPACING;
    let mut y = scroll.y() + DIALOG_SPACING;
    let usable = state.row_width;

    scroll.begin();
    match parsed {
        Ok(data) => {
            let mapping = if has_header {
                default_mapping(&data.columns, &state.targets)
            } else {
                state.table_columns.positional_mapping(data.columns.len())
            };
            for (index, column) in data.columns.iter().enumerate() {
                let mut name = Frame::new(x, y, MAPPING_NAME_WIDTH, MAPPING_ROW_HEIGHT, None);
                name.set_label(&elide(column, 34));
                name.set_label_color(theme::text_primary());
                name.set_align(Align::Inside | Align::Left);
                name.set_tooltip(column);

                let choice_x = x + MAPPING_NAME_WIDTH + DIALOG_SPACING;
                let choice_w = (usable - MAPPING_NAME_WIDTH - DIALOG_SPACING).max(120);
                let mut choice = Choice::new(choice_x, y, choice_w, MAPPING_ROW_HEIGHT, None);
                // ONE item per target, whatever a column is called: the
                // mapping this dialog returns is the item INDEX, so a name that
                // became two entries mapped every later column onto its
                // neighbour — the user picked NAME and the import wrote into
                // the next one.
                add_menu_item(&mut choice, "(skip)");
                for target in &state.targets {
                    add_menu_item(
                        &mut choice,
                        &format!(
                            "{}  ·  {}",
                            target.name,
                            if target.nullable {
                                "null ok"
                            } else {
                                "required"
                            }
                        ),
                    );
                }
                choice.set_value(
                    mapping
                        .get(index)
                        .copied()
                        .flatten()
                        .and_then(|target| i32::try_from(target + 1).ok())
                        .unwrap_or(0),
                );
                theme::style_choice(&mut choice);
                theme::install_choice_hover(&mut choice);
                choice.set_callback(|_| notify_summary_refresh());
                choices.push(choice);

                y += MAPPING_ROW_HEIGHT + 2;
            }
        }
        Err(error) => {
            let mut message = Frame::new(x, y, usable, MAPPING_ROW_HEIGHT * 2, None);
            message.set_label(error);
            message.set_label_color(theme::text_error());
            message.set_align(Align::Inside | Align::Left | Align::Wrap);
        }
    }
    scroll.end();
    *state
        .mapping_choices
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = choices;
    scroll.scroll_to(0, 0);
    scroll.redraw();
}

fn update_summary(state: &DialogState, summary: &mut Frame) {
    let parsed = state
        .parsed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let text = match &*parsed {
        Err(error) => {
            summary.set_label_color(theme::text_error());
            error.clone()
        }
        Ok(data) => {
            summary.set_label_color(theme::text_secondary());
            let mapping = state.mapping();
            let mapped = mapping.iter().filter(|target| target.is_some()).count();
            let unmapped: Vec<&str> = state
                .targets
                .iter()
                .enumerate()
                .filter(|(index, target)| !target.nullable && !mapping.contains(&Some(*index)))
                .map(|(_, target)| target.name.as_str())
                .collect();
            let mut text = format!(
                "{} row(s), {mapped} of {} file column(s) mapped.",
                data.rows.len(),
                data.columns.len()
            );
            if !unmapped.is_empty() {
                text.push_str(&format!(
                    "\nNot mapped and not nullable: {}",
                    elide(&unmapped.join(", "), 80)
                ));
            }
            if !state.generated_columns.is_empty() {
                text.push_str(&format!(
                    "\nComputed by the server, so not listed: {}",
                    elide(&state.generated_columns.join(", "), 80)
                ));
            }
            text
        }
    };
    summary.set_label(&text);
    summary.redraw();
}

fn elide(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

/// A mapping Choice has to refresh the summary, but the summary lives in the
/// closure that built it. One modal is open at a time, so a slot holding that
/// closure for the life of the dialog is enough.
type SummaryRefresh = Box<dyn FnMut() + Send>;
static SUMMARY_REFRESH: Mutex<Option<SummaryRefresh>> = Mutex::new(None);

fn notify_summary_refresh() {
    // take → unlock → invoke → restore, like every other callback slot: the
    // guard may not be held while the closure runs, or a refresh that asks
    // for another refresh deadlocks the UI thread on this slot.
    let mut refresh = SUMMARY_REFRESH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(refresh) = refresh.as_mut() {
        refresh();
    }
    let mut slot = SUMMARY_REFRESH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.is_none() {
        *slot = refresh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    /// The slot's guard may not be held while the refresh closure runs: a
    /// refresh that (transitively) asks for another refresh would deadlock the
    /// UI thread on this slot. Runs on a worker thread so a regression fails
    /// this test by timeout instead of hanging the whole run.
    #[test]
    fn summary_refresh_slot_is_not_held_while_the_refresh_closure_runs() {
        let (done_sender, done_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let reentered = Arc::new(AtomicBool::new(false));
            let reentered_in_refresh = Arc::clone(&reentered);
            *SUMMARY_REFRESH
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(move || {
                // Once, not recursively: the point is one nested call.
                if !reentered_in_refresh.swap(true, Ordering::AcqRel) {
                    notify_summary_refresh();
                }
            }));
            notify_summary_refresh();
            let _ = done_sender.send(());
        });
        done_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("a re-entrant summary refresh must not deadlock on the slot lock");
        worker.join().expect("summary refresh worker");
        let mut slot = SUMMARY_REFRESH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            slot.is_some(),
            "the refresh closure must be back in the slot after the call"
        );
        *slot = None;
    }
}
