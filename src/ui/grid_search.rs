//! Find text inside the rows a result grid is already showing.
//!
//! This searches the fetched values only — nothing is sent to the server and no
//! statement is re-run. That is what separates it from the result filter, which
//! re-queries: a filter removes rows, this one leaves every row in place and
//! points at the cells that match.
//!
//! The substring search itself is `find_replace::find_next_match`, the same
//! routine the editor's Find uses, so case folding and UTF-8 boundaries behave
//! identically in both places.

use crate::ui::center_on_main;
use crate::ui::constants::*;
use crate::ui::find_replace::{find_next_match, install_find_input_shortcuts};
use crate::ui::result_table::ResultTableWidget;
use crate::ui::theme;
use crate::utils::arithmetic::safe_rem;
use fltk::{
    app,
    button::{Button, CheckButton},
    enums::{CallbackTrigger, FrameType, Shortcut},
    frame::Frame,
    group::Flex,
    input::Input,
    prelude::*,
    window::Window,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Which cells the grid paints as search hits, and which one of them is the
/// match the user is standing on.
#[derive(Default)]
pub(crate) struct GridSearchHighlight {
    pub(crate) matches: HashSet<(usize, usize)>,
    pub(crate) current: Option<(usize, usize)>,
}

impl GridSearchHighlight {
    pub(crate) fn clear(&mut self) {
        self.matches.clear();
        self.matches.shrink_to_fit();
        self.current = None;
    }
}

/// Every cell whose value contains `needle`, in reading order (row, then
/// column).
///
/// `hidden_col` is the grid's zero-width ROWID column when edit mode put one
/// there. It holds a value the user never sees, so a hit in it would move the
/// selection to a cell that cannot be scrolled into view.
pub(crate) fn grid_matches(
    rows: &[Vec<String>],
    needle: &str,
    case_sensitive: bool,
    hidden_col: Option<usize>,
) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, value) in row.iter().enumerate() {
            if hidden_col == Some(col_index) {
                continue;
            }
            if find_next_match(value, needle, 0, case_sensitive).is_some() {
                matches.push((row_index, col_index));
            }
        }
    }
    matches
}

/// The first match at or after `origin`, wrapping to the first match when the
/// origin sits past the last one.
///
/// `origin` is the cell the user was on when the needle changed, so a fresh
/// search starts where they are looking instead of jumping back to row 1.
pub(crate) fn first_match_at_or_after(
    matches: &[(usize, usize)],
    origin: Option<(usize, usize)>,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let Some(origin) = origin else {
        return Some(0);
    };
    Some(matches.iter().position(|cell| *cell >= origin).unwrap_or(0))
}

/// The neighbouring match, wrapping at both ends.
pub(crate) fn stepped_match(len: usize, current: Option<usize>, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current.filter(|index| *index < len) else {
        return Some(if forward { 0 } else { len - 1 });
    };
    Some(if forward {
        safe_rem(current + 1, len)
    } else {
        safe_rem(current + len - 1, len)
    })
}

/// What the dialog prints next to the buttons.
pub(crate) fn match_status(matches_len: usize, current: Option<usize>, needle: &str) -> String {
    if needle.is_empty() {
        return String::new();
    }
    if matches_len == 0 {
        return "No matches".to_string();
    }
    match current {
        Some(index) => format!("{} of {}", index + 1, matches_len),
        None => format!("{} matches", matches_len),
    }
}

/// One find session: what was searched for, what it found, and where the user
/// is standing in the result.
#[derive(Default)]
struct GridSearchSession {
    matches: Vec<(usize, usize)>,
    current: Option<usize>,
    /// The cell a fresh needle starts from — the selection at first, then the
    /// match the user last stepped to.
    origin: Option<(usize, usize)>,
    /// Needle, case flag, and the grid's data generation the matches came from.
    applied: Option<(String, bool, u64)>,
}

/// Move to the next or previous match, re-scanning first when the needle, the
/// case flag, or the rows themselves have changed since the last scan.
///
/// Returns the text for the dialog's status label.
fn step_search(
    table: &mut ResultTableWidget,
    session: &mut GridSearchSession,
    needle: &str,
    case_sensitive: bool,
    forward: bool,
) -> String {
    let query = (needle.to_string(), case_sensitive, table.data_generation());
    if session.applied.as_ref() != Some(&query) {
        session.matches = table.search_matches(needle, case_sensitive);
        session.current = first_match_at_or_after(&session.matches, session.origin);
        session.applied = Some(query);
    } else {
        session.current = stepped_match(session.matches.len(), session.current, forward);
    }

    let focused = session
        .current
        .and_then(|index| session.matches.get(index))
        .copied();
    table.set_search_highlight(&session.matches, focused);
    if let Some(cell) = focused {
        session.origin = Some(cell);
        table.focus_search_cell(cell.0, cell.1);
    }
    match_status(session.matches.len(), session.current, needle)
}

/// Wire `widget` to step the search when it fires.
///
/// `forward` is `None` for the text input, where the direction comes from the
/// Shift key so Enter and Shift+Enter mirror the two buttons.
fn install_step<W: WidgetExt>(
    widget: &mut W,
    forward: Option<bool>,
    table: &ResultTableWidget,
    session: &Arc<Mutex<GridSearchSession>>,
    find_input: &Input,
    case_check: &CheckButton,
    status_label: &Frame,
) {
    let mut table = table.clone();
    let session = session.clone();
    let find_input = find_input.clone();
    let case_check = case_check.clone();
    let mut status_label = status_label.clone();
    widget.set_callback(move |_| {
        let forward = forward.unwrap_or_else(|| !app::event_state().contains(Shortcut::Shift));
        let mut session = session
            .lock()
            .unwrap_or_else(|poisoned: std::sync::PoisonError<_>| poisoned.into_inner());
        let status = step_search(
            &mut table,
            &mut session,
            &find_input.value(),
            case_check.value(),
            forward,
        );
        status_label.set_label(&status);
        status_label.redraw_label();
    });
}

pub(crate) struct GridSearchDialog;

impl GridSearchDialog {
    /// Run the modal find-in-results dialog against `table`.
    ///
    /// Every button acts inside its own callback rather than posting to the
    /// loop below, so a step still happens when the dialog is driven from a
    /// nested event context.
    ///
    /// The highlight belongs to the dialog: whichever way it is closed, the
    /// grid is left without search tinting, so a stale highlight can never
    /// outlive the search that produced it.
    pub(crate) fn show(table: &mut ResultTableWidget) {
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let height = 130;
        let mut dialog = Window::default()
            .with_size(450, height)
            .with_label("Find in Results");
        center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);

        let mut main_flex = Flex::default().with_pos(10, 10).with_size(430, height - 20);
        main_flex.set_type(fltk::group::FlexType::Column);
        main_flex.set_spacing(DIALOG_SPACING);

        let mut find_flex = Flex::default();
        find_flex.set_type(fltk::group::FlexType::Row);
        let mut find_label = Frame::default().with_label("Find:");
        find_label.set_label_color(theme::text_primary());
        find_flex.fixed(&find_label, FORM_LABEL_WIDTH);
        let mut find_input = Input::default();
        find_input.set_color(theme::input_bg());
        find_input.set_text_color(theme::text_primary());
        theme::apply_text_input_inset(&mut find_input);
        find_input.set_trigger(CallbackTrigger::EnterKeyAlways);
        install_find_input_shortcuts(&mut find_input);
        find_flex.end();
        main_flex.fixed(&find_flex, INPUT_ROW_HEIGHT);

        let mut options_flex = Flex::default();
        options_flex.set_type(fltk::group::FlexType::Row);
        let mut case_check = CheckButton::default().with_label("Case sensitive");
        case_check.set_color(theme::button_dark());
        case_check.set_label_color(theme::text_secondary());
        theme::install_button_hover(&mut case_check);
        let mut status_label = Frame::default();
        status_label.set_label_color(theme::text_secondary());
        options_flex.end();
        main_flex.fixed(&options_flex, CHECKBOX_ROW_HEIGHT);

        let mut button_flex = Flex::default();
        button_flex.set_type(fltk::group::FlexType::Row);
        button_flex.set_spacing(DIALOG_SPACING);
        let _spacer = Frame::default();

        let mut previous_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Previous");
        previous_btn.set_color(theme::button_dark());
        previous_btn.set_label_color(theme::text_primary());
        previous_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut previous_btn);

        let mut next_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Next");
        next_btn.set_color(theme::button_dark());
        next_btn.set_label_color(theme::text_primary());
        next_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut next_btn);

        let mut close_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Close");
        close_btn.set_color(theme::button_dark());
        close_btn.set_label_color(theme::text_primary());
        close_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut close_btn);

        button_flex.fixed(&previous_btn, BUTTON_WIDTH);
        button_flex.fixed(&next_btn, BUTTON_WIDTH);
        button_flex.fixed(&close_btn, BUTTON_WIDTH_SMALL);
        button_flex.end();
        main_flex.fixed(&button_flex, BUTTON_ROW_HEIGHT);

        main_flex.end();
        dialog.end();
        fltk::group::Group::set_current(current_group.as_ref());

        let session = Arc::new(Mutex::new(GridSearchSession {
            // The cell the user was standing on seeds the first search.
            origin: table.selected_cell(),
            ..GridSearchSession::default()
        }));

        install_step(
            &mut next_btn,
            Some(true),
            table,
            &session,
            &find_input,
            &case_check,
            &status_label,
        );
        install_step(
            &mut previous_btn,
            Some(false),
            table,
            &session,
            &find_input,
            &case_check,
            &status_label,
        );
        let mut input_for_enter = find_input.clone();
        install_step(
            &mut input_for_enter,
            None,
            table,
            &session,
            &find_input,
            &case_check,
            &status_label,
        );

        let mut dialog_for_close = dialog.clone();
        close_btn.set_callback(move |_| {
            dialog_for_close.hide();
        });

        dialog.show();
        let _ = find_input.take_focus();
        while dialog.shown() {
            app::wait();
        }

        table.clear_search_highlight();
    }
}

#[cfg(test)]
mod tests {
    use super::{first_match_at_or_after, grid_matches, match_status, stepped_match};

    fn sample_rows() -> Vec<Vec<String>> {
        vec![
            vec!["7369".to_string(), "SMITH".to_string(), "CLERK".to_string()],
            vec!["7499".to_string(), "ALLEN".to_string(), "SALES".to_string()],
            vec![
                "7521".to_string(),
                "smithers".to_string(),
                "CLERK".to_string(),
            ],
        ]
    }

    #[test]
    fn grid_matches_reports_cells_in_reading_order() {
        let matches = grid_matches(&sample_rows(), "CLERK", true, None);
        assert_eq!(matches, vec![(0, 2), (2, 2)]);
    }

    #[test]
    fn grid_matches_folds_case_when_case_sensitivity_is_off() {
        let matches = grid_matches(&sample_rows(), "smith", false, None);
        assert_eq!(matches, vec![(0, 1), (2, 1)]);

        let case_sensitive = grid_matches(&sample_rows(), "smith", true, None);
        assert_eq!(case_sensitive, vec![(2, 1)]);
    }

    #[test]
    fn grid_matches_skips_the_zero_width_rowid_column() {
        let rows = vec![vec!["AAAR3s".to_string(), "AAAR3s-visible".to_string()]];
        let matches = grid_matches(&rows, "AAAR3s", true, Some(0));
        assert_eq!(matches, vec![(0, 1)]);
    }

    #[test]
    fn grid_matches_is_empty_for_an_empty_needle() {
        assert!(grid_matches(&sample_rows(), "", false, None).is_empty());
    }

    #[test]
    fn grid_matches_finds_substrings_inside_multibyte_values() {
        let rows = vec![vec!["서울특별시 강남구".to_string()]];
        assert_eq!(grid_matches(&rows, "강남", true, None), vec![(0, 0)]);
    }

    #[test]
    fn first_match_starts_at_the_cell_the_user_is_standing_on() {
        let matches = vec![(0, 2), (2, 2), (5, 1)];
        assert_eq!(first_match_at_or_after(&matches, Some((2, 0))), Some(1));
        assert_eq!(first_match_at_or_after(&matches, Some((0, 2))), Some(0));
    }

    #[test]
    fn first_match_wraps_when_the_origin_is_past_the_last_match() {
        let matches = vec![(0, 2), (2, 2)];
        assert_eq!(first_match_at_or_after(&matches, Some((9, 9))), Some(0));
        assert_eq!(first_match_at_or_after(&[], Some((0, 0))), None);
        assert_eq!(first_match_at_or_after(&matches, None), Some(0));
    }

    #[test]
    fn stepped_match_wraps_at_both_ends() {
        assert_eq!(stepped_match(3, Some(2), true), Some(0));
        assert_eq!(stepped_match(3, Some(0), false), Some(2));
        assert_eq!(stepped_match(3, None, true), Some(0));
        assert_eq!(stepped_match(3, None, false), Some(2));
        assert_eq!(stepped_match(0, Some(1), true), None);
    }

    #[test]
    fn stepped_match_recovers_from_an_index_left_over_from_a_longer_result() {
        assert_eq!(stepped_match(2, Some(7), true), Some(0));
    }

    #[test]
    fn match_status_reports_position_count_and_absence() {
        assert_eq!(match_status(0, None, ""), "");
        assert_eq!(match_status(0, None, "smith"), "No matches");
        assert_eq!(match_status(17, Some(2), "smith"), "3 of 17");
        assert_eq!(match_status(17, None, "smith"), "17 matches");
    }
}
