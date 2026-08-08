//! Filter a result grid down to the values it is already showing.
//!
//! This is the counterpart to the `WHERE` / `ORDER BY` bar, and the two are
//! deliberately exclusive: a result the app can re-query gets the bar, whose
//! semantics are the server's and therefore exact; a result it cannot re-query
//! — a script product, a statement carrying binds or substitution variables, a
//! MySQL join repeating a column name, a grid whose connection is gone — gets
//! this instead. Offering both on one grid would leave the user unable to tell
//! which set of rules produced the rows in front of them.
//!
//! What makes a local filter defensible here, when [`crate::ui::result_filter`]
//! rejected a local evaluator outright, is that this one never evaluates an
//! expression. There is no parsing, no type coercion, no function support and
//! no collation: the user points at cells, and a row is kept when its text in
//! those columns is byte-for-byte what was pointed at. NULL is the grid's own
//! NULL, read through the same rule the rest of the grid uses.
//!
//! One consequence is worth stating because it differs from SQL: locally the
//! exclusion really is the exact complement. A displayed cell is either NULL or
//! a string, so there is no UNKNOWN for a row to fall into, and the two
//! directions always partition the rows between them.

use fltk::button::Button;
use fltk::enums::{Align, FrameType};
use fltk::frame::Frame;
use fltk::group::Group;
use fltk::prelude::*;

use crate::ui::column_layout::HiddenColumns;
use crate::ui::result_table::ResultTableWidget;
use crate::ui::table_browse::TABLE_BROWSE_FILTER_HEIGHT;
use crate::ui::theme;

/// How long a value may be before the filter description abbreviates it.
const DESCRIPTION_VALUE_LIMIT: usize = 40;

/// One cell of a filter key, resolved against the grid's NULL rule once so
/// matching never re-derives it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CellKey {
    Null,
    Value(String),
}

impl CellKey {
    fn from_cell(value: &str, null_text: &str) -> Self {
        if ResultTableWidget::value_represents_null(value, null_text) {
            Self::Null
        } else {
            Self::Value(value.to_string())
        }
    }

    fn matches(&self, value: &str, null_text: &str) -> bool {
        match self {
            Self::Null => ResultTableWidget::value_represents_null(value, null_text),
            Self::Value(expected) => {
                !ResultTableWidget::value_represents_null(value, null_text) && expected == value
            }
        }
    }
}

/// A filter built from a grid selection.
///
/// The shape mirrors the `Where Clause` export: a row is kept when it matches
/// *every* selected column of *some* selected row, which is the same
/// `(a AND b) OR (c AND d)` grouping that export emits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridValueFilter {
    /// Data column indexes the selection covered, ascending, hidden columns
    /// already dropped.
    columns: Vec<usize>,
    /// One entry per distinct selected row, aligned to `columns`.
    keys: Vec<Vec<CellKey>>,
    /// The grid's NULL display text, captured so matching cannot drift from the
    /// text the keys were read through.
    null_text: String,
    negate: bool,
}

impl GridValueFilter {
    /// Whether `row` survives the filter.
    pub fn matches(&self, row: &[String]) -> bool {
        let hit = self.keys.iter().any(|key_row| {
            self.columns
                .iter()
                .zip(key_row.iter())
                .all(|(column, key)| {
                    key.matches(row.get(*column).map_or("", String::as_str), &self.null_text)
                })
        });
        hit != self.negate
    }

    /// The rows of `rows` that survive, in their original order.
    pub fn retain(&self, rows: &[Vec<String>]) -> Vec<Vec<String>> {
        rows.iter()
            .filter(|row| self.matches(row))
            .cloned()
            .collect()
    }

    /// A one-line account of what is being filtered, for the strip above the
    /// grid. Says which direction it runs in, because a filter the user cannot
    /// see is a filter that will be blamed on the data.
    pub fn describe(&self, headers: &[String]) -> String {
        let verb = if self.negate { "Hiding" } else { "Showing" };
        let condition = match (self.columns.as_slice(), self.keys.len()) {
            ([column], 1) => {
                let name = Self::column_name(headers, *column);
                match self.keys.first().and_then(|row| row.first()) {
                    Some(CellKey::Null) => format!("{name} is NULL"),
                    Some(CellKey::Value(value)) => {
                        format!("{name} = {}", Self::abbreviate(value))
                    }
                    None => name,
                }
            }
            ([column], count) => {
                format!(
                    "{} is one of {count} values",
                    Self::column_name(headers, *column)
                )
            }
            (columns, 1) => format!("{} columns match 1 row", columns.len()),
            (columns, count) => format!("{} columns match {count} rows", columns.len()),
        };
        format!("{verb} rows where {condition}")
    }

    fn column_name(headers: &[String], column: usize) -> String {
        headers
            .get(column)
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map_or_else(|| format!("column {}", column + 1), str::to_string)
    }

    /// Shorten a value for the description without letting a multi-byte
    /// character be cut in half.
    fn abbreviate(value: &str) -> String {
        let mut out: String = value.chars().take(DESCRIPTION_VALUE_LIMIT).collect();
        if out.chars().count() < value.chars().count() {
            out.push('\u{2026}');
        }
        out
    }
}

/// Build a filter from a selection rectangle over `rows`.
///
/// Returns `None` when the rectangle covers nothing usable — no rows, or only
/// the hidden technical column — so the caller never installs a filter that
/// would keep everything or nothing for no stated reason.
pub fn build(
    rows: &[Vec<String>],
    bounds: (usize, usize, usize, usize),
    hidden_col: &HiddenColumns,
    null_text: &str,
    negate: bool,
) -> Option<GridValueFilter> {
    let (row_start, col_start, row_end, col_end) = bounds;
    if row_start > row_end || col_start > col_end {
        return None;
    }
    let columns: Vec<usize> = (col_start..=col_end)
        .filter(|column| !hidden_col.contains(*column))
        .collect();
    if columns.is_empty() {
        return None;
    }

    let mut keys: Vec<Vec<CellKey>> = Vec::new();
    for row in rows.iter().take(row_end + 1).skip(row_start) {
        let key_row: Vec<CellKey> = columns
            .iter()
            .map(|column| {
                CellKey::from_cell(row.get(*column).map_or("", String::as_str), null_text)
            })
            .collect();
        if !keys.contains(&key_row) {
            keys.push(key_row);
        }
    }
    if keys.is_empty() {
        return None;
    }

    Some(GridValueFilter {
        columns,
        keys,
        null_text: null_text.to_string(),
        negate,
    })
}

/// The strip that reports an active value filter above the grid.
///
/// It sits in the slot the `WHERE` / `ORDER BY` bar would occupy, which is free
/// precisely because a grid gets one or the other. It is a label and a clear
/// button, deliberately not an input: accepting typed conditions here would put
/// back the local expression evaluator this app decided not to own.
#[derive(Clone)]
pub struct GridValueFilterBar {
    group: Group,
    label: Frame,
    clear: Button,
}

impl GridValueFilterBar {
    pub fn new(x: i32, y: i32, w: i32) -> Self {
        let mut group = Group::new(x, y, w.max(1), TABLE_BROWSE_FILTER_HEIGHT, None);
        group.set_frame(FrameType::FlatBox);
        group.set_color(theme::panel_bg());
        group.begin();

        let mut label = Frame::default();
        label.set_align(Align::Left | Align::Inside);
        label.set_label_color(theme::text_secondary());

        let mut clear = Button::default().with_label("\u{d7}");
        clear.set_color(theme::button_subtle());
        clear.set_label_color(theme::text_secondary());
        clear.set_frame(FrameType::RFlatBox);
        clear.set_tooltip("Remove the value filter and show every fetched row");
        theme::install_button_hover(&mut clear);

        group.end();

        let mut bar = Self {
            group,
            label,
            clear,
        };
        bar.layout(x, y, w);
        bar
    }

    pub fn layout(&mut self, x: i32, y: i32, w: i32) {
        const HORIZONTAL_PADDING: i32 = 8;
        const VERTICAL_PADDING: i32 = 8;
        const GAP: i32 = 6;
        const CLEAR_WIDTH: i32 = 24;

        self.group
            .resize(x, y, w.max(1), TABLE_BROWSE_FILTER_HEIGHT);
        let control_y = y + VERTICAL_PADDING;
        let control_h = TABLE_BROWSE_FILTER_HEIGHT - VERTICAL_PADDING * 2;
        let label_width = (w - HORIZONTAL_PADDING * 2 - CLEAR_WIDTH - GAP).max(40);
        self.label
            .resize(x + HORIZONTAL_PADDING, control_y, label_width, control_h);
        self.clear.resize(
            x + HORIZONTAL_PADDING + label_width + GAP,
            control_y,
            CLEAR_WIDTH,
            control_h,
        );
        self.group.redraw();
    }

    /// Report what is filtered and how much of the result survived it.
    ///
    /// The row counts are the point: a filter the user forgot about looks
    /// exactly like a query that returned little.
    pub fn set_state(&mut self, description: &str, kept_rows: usize, total_rows: usize) {
        self.label.set_label(&format!(
            "{description} \u{2014} {kept_rows} of {total_rows} rows"
        ));
        self.group.redraw();
    }

    pub fn set_clear_callback<F: FnMut() + 'static>(&mut self, mut callback: F) {
        self.clear.set_callback(move |_| callback());
    }

    pub fn cleanup_for_close(&mut self) {
        self.clear.set_callback(|_| {});
    }

    /// The group this strip owns, so the caller can delete it when the filter
    /// goes away and the grid takes the space back.
    pub fn take_group(self) -> Group {
        self.group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NULL_TEXT: &str = "NULL";

    fn grid() -> Vec<Vec<String>> {
        [
            ["7369", "SMITH", "CLERK"],
            ["7499", "ALLEN", "SALESMAN"],
            ["7521", "SMITH", "SALESMAN"],
            ["7566", "NULL", "MANAGER"],
        ]
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect()
    }

    fn headers() -> Vec<String> {
        ["EMPNO", "ENAME", "JOB"]
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    }

    fn kept(filter: &GridValueFilter, rows: &[Vec<String>]) -> Vec<String> {
        filter
            .retain(rows)
            .into_iter()
            .map(|row| row[0].clone())
            .collect()
    }

    #[test]
    fn one_cell_keeps_every_row_sharing_that_value() {
        let rows = grid();
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7369", "7521"]);
    }

    #[test]
    fn excluding_one_cell_keeps_exactly_the_other_rows() {
        let rows = grid();
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            true,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7499", "7566"]);
    }

    #[test]
    fn the_two_directions_partition_every_row() {
        // Locally there is no UNKNOWN to fall into, so this holds for every
        // selection — including one that pins a NULL, where the SQL form needs
        // a deliberate IS NULL / IS NOT NULL split to manage the same thing.
        let rows = grid();
        for bounds in [(0, 1, 0, 1), (3, 1, 3, 1), (0, 0, 1, 2), (0, 1, 3, 1)] {
            let keep =
                build(&rows, bounds, &HiddenColumns::default(), NULL_TEXT, false).expect("filter");
            let drop =
                build(&rows, bounds, &HiddenColumns::default(), NULL_TEXT, true).expect("filter");
            for row in &rows {
                assert_ne!(
                    keep.matches(row),
                    drop.matches(row),
                    "row {row:?} at {bounds:?} matched both or neither"
                );
            }
        }
    }

    #[test]
    fn a_null_cell_matches_only_the_other_nulls() {
        let rows = grid();
        let filter = build(
            &rows,
            (3, 1, 3, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7566"]);
        assert_eq!(
            filter.describe(&headers()),
            "Showing rows where ENAME is NULL"
        );
    }

    #[test]
    fn an_empty_cell_is_the_same_null_the_grid_shows() {
        // value_represents_null treats both the configured text and an empty
        // cell as NULL, and the filter must not invent a third answer.
        let rows: Vec<Vec<String>> = [["a", ""], ["b", "NULL"], ["c", "x"]]
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_string()).collect())
            .collect();
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["a", "b"]);
    }

    #[test]
    fn several_cells_in_one_column_collect_into_a_value_set() {
        let rows = grid();
        let filter = build(
            &rows,
            (0, 1, 1, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7369", "7499", "7521"]);
        assert_eq!(
            filter.describe(&headers()),
            "Showing rows where ENAME is one of 2 values"
        );
    }

    #[test]
    fn several_columns_must_all_match_within_one_selected_row() {
        let rows = grid();
        // ENAME + JOB of row 0 only; SMITH/SALESMAN must not be pulled in.
        let filter = build(
            &rows,
            (0, 1, 0, 2),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7369"]);
        assert_eq!(
            filter.describe(&headers()),
            "Showing rows where 2 columns match 1 row"
        );
    }

    #[test]
    fn repeated_selected_rows_collapse_into_one_key() {
        let rows = grid();
        // Rows 0 and 2 both hold SMITH, so the value set has one entry.
        let filter = build(
            &rows,
            (0, 1, 2, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(
            filter.describe(&headers()),
            "Showing rows where ENAME is one of 2 values"
        );
        assert_eq!(kept(&filter, &rows), vec!["7369", "7499", "7521"]);
    }

    #[test]
    fn matching_is_case_sensitive_and_exact() {
        let rows: Vec<Vec<String>> = [["a", "Smith"], ["b", "SMITH"], ["c", " SMITH"]]
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_string()).collect())
            .collect();
        let filter = build(
            &rows,
            (1, 1, 1, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["b"]);
    }

    #[test]
    fn the_hidden_technical_column_is_never_part_of_a_filter() {
        let rows = grid();
        // Selecting across column 0 while it is the hidden ROWID leaves ENAME.
        let filter = build(
            &rows,
            (0, 0, 0, 1),
            &HiddenColumns::automatic(Some(0)),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["7369", "7521"]);
        // A selection of nothing but the hidden column builds no filter at all.
        assert!(build(
            &rows,
            (0, 0, 0, 0),
            &HiddenColumns::automatic(Some(0)),
            NULL_TEXT,
            false
        )
        .is_none());
    }

    #[test]
    fn an_empty_rectangle_builds_nothing() {
        let rows = grid();
        assert!(build(
            &rows,
            (1, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false
        )
        .is_none());
        assert!(build(
            &[],
            (0, 0, 0, 0),
            &HiddenColumns::default(),
            NULL_TEXT,
            false
        )
        .is_none());
    }

    #[test]
    fn a_short_row_is_treated_as_holding_null_there() {
        // Streaming can leave a row shorter than the header list; reading past
        // its end must not panic and must not match a real value.
        let rows: Vec<Vec<String>> = vec![
            vec!["a".to_string(), "x".to_string()],
            vec!["b".to_string()],
        ];
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["a"]);
        let filter = build(
            &rows,
            (1, 1, 1, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        assert_eq!(kept(&filter, &rows), vec!["b"]);
    }

    #[test]
    fn a_long_value_is_abbreviated_without_splitting_a_character() {
        let long = "한".repeat(60);
        let rows: Vec<Vec<String>> = vec![vec!["a".to_string(), long.clone()]];
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            false,
        )
        .expect("filter");
        let described = filter.describe(&headers());
        assert!(described.ends_with('\u{2026}'));
        assert_eq!(described.chars().filter(|c| *c == '한').count(), 40);
    }

    #[test]
    fn a_blank_header_is_named_by_position() {
        let rows = grid();
        let headers = vec!["EMPNO".to_string(), "   ".to_string()];
        let filter = build(
            &rows,
            (0, 1, 0, 1),
            &HiddenColumns::default(),
            NULL_TEXT,
            true,
        )
        .expect("filter");
        assert_eq!(
            filter.describe(&headers),
            "Hiding rows where column 2 = SMITH"
        );
    }
}
