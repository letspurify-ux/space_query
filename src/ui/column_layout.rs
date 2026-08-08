//! Which grid columns are shown, and in what order.
//!
//! The grid keeps "display column" and "data column" as the same integer
//! everywhere — the draw callback, every hit test, fifteen keyboard-navigation
//! helpers, the staged-edit session, search, and the selection summary all index
//! one vector by one number. Rather than thread a display-to-data map through
//! all of that, reordering physically permutes the stored columns, so that
//! invariant survives untouched and only the permutation itself is new.
//!
//! This module owns the permutation: what the user asked for, whether it is
//! legal, and how to apply it to a parallel vector. It holds no widgets, so the
//! rules are testable without a window.
//!
//! One column may be `locked`: the technical ROWID the grid hides to make a
//! result editable. It is never listed, never moved and never shown, but it has
//! to keep its index while everything around it moves — so it is planned here
//! rather than filtered out and lost.

use std::collections::HashSet;

/// Which columns a grid is not drawing.
///
/// Two sources, one answer: the technical ROWID the grid hides on its own, and
/// whatever the user hid in the Columns dialog. Every place that used to ask
/// "is this *the* hidden column" asks this instead, so neither source can be
/// forgotten at one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HiddenColumns {
    /// The technical column the grid manages itself, if any.
    auto: Option<usize>,
    /// Columns the user hid.
    user: std::collections::HashSet<usize>,
}

impl HiddenColumns {
    pub fn new(auto: Option<usize>, user: HashSet<usize>) -> Self {
        Self { auto, user }
    }

    /// Only the grid's own technical column, for paths that have no user
    /// layout to consult.
    #[cfg(test)]
    pub fn automatic(auto: Option<usize>) -> Self {
        Self {
            auto,
            user: HashSet::new(),
        }
    }

    pub fn contains(&self, column: usize) -> bool {
        self.auto == Some(column) || self.user.contains(&column)
    }

    pub fn is_empty(&self) -> bool {
        self.auto.is_none() && self.user.is_empty()
    }

    pub fn automatic_column(&self) -> Option<usize> {
        self.auto
    }

    /// Every hidden column, so the caller can set each width to zero.
    pub fn all(&self) -> Vec<usize> {
        let mut columns: Vec<usize> = self.user.iter().copied().collect();
        if let Some(auto) = self.auto {
            if !self.user.contains(&auto) {
                columns.push(auto);
            }
        }
        columns.sort_unstable();
        columns
    }
}

/// One column, as the Columns dialog sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnLayoutRow {
    /// Where this column sits in the grid right now.
    pub grid_index: usize,
    /// Where it sat when the result arrived, so Reset can undo every change
    /// made since — including ones from an earlier visit to the dialog.
    pub source_index: usize,
    pub name: String,
    pub visible: bool,
    /// A technical column the grid manages itself. Not listed, not movable.
    pub locked: bool,
}

/// A proposed arrangement of every column in one grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnLayoutPlan {
    /// Movable columns in the order they will be displayed.
    movable: Vec<ColumnLayoutRow>,
    /// Locked columns, each keeping the absolute index it already has.
    locked: Vec<ColumnLayoutRow>,
}

impl ColumnLayoutPlan {
    /// Start from what the grid is showing now.
    pub fn from_rows(rows: Vec<ColumnLayoutRow>) -> Self {
        let (locked, movable) = rows.into_iter().partition(|row| row.locked);
        Self { movable, locked }
    }

    /// The columns the dialog lists, in display order.
    pub fn rows(&self) -> &[ColumnLayoutRow] {
        &self.movable
    }

    pub fn visible_count(&self) -> usize {
        self.movable.iter().filter(|row| row.visible).count()
    }

    /// Move the row at `at` one place toward the front or back.
    ///
    /// Returns false at the ends rather than wrapping: a Move Up on the first
    /// column should do nothing, not send it to the bottom.
    pub fn move_row(&mut self, at: usize, forward: bool) -> Option<usize> {
        if at >= self.movable.len() {
            return None;
        }
        let target = if forward {
            at.checked_add(1)
                .filter(|next| *next < self.movable.len())?
        } else {
            at.checked_sub(1)?
        };
        self.movable.swap(at, target);
        Some(target)
    }

    /// Show or hide one column.
    ///
    /// Hiding the last visible column is refused: an empty grid has no cell to
    /// right-click, so there would be no way back to this dialog.
    pub fn set_visible(&mut self, at: usize, visible: bool) -> Result<(), String> {
        let Some(row) = self.movable.get_mut(at) else {
            return Err("That column is no longer in the result.".to_string());
        };
        if row.visible == visible {
            return Ok(());
        }
        if !visible && self.visible_count() <= 1 {
            return Err("At least one column has to stay visible.".to_string());
        }
        let Some(row) = self.movable.get_mut(at) else {
            return Err("That column is no longer in the result.".to_string());
        };
        row.visible = visible;
        Ok(())
    }

    /// Put every column back where the result put it, all visible.
    pub fn reset(&mut self) {
        self.movable.sort_by_key(|row| row.source_index);
        for row in &mut self.movable {
            row.visible = true;
        }
    }

    fn column_count(&self) -> usize {
        self.movable.len() + self.locked.len()
    }

    fn hidden_grid_indices(&self) -> HashSet<usize> {
        self.movable
            .iter()
            .filter(|row| !row.visible)
            .map(|row| row.grid_index)
            .collect()
    }

    /// The permutation to apply: for each new display position, the grid index
    /// currently holding that column.
    ///
    /// Locked columns keep their absolute index; the movable ones fill what is
    /// left, in the order the user arranged them.
    pub fn order(&self) -> Vec<usize> {
        let total = self.column_count();
        let mut order = vec![usize::MAX; total];
        for row in &self.locked {
            if row.grid_index < total {
                order[row.grid_index] = row.grid_index;
            }
        }
        let mut movable = self.movable.iter();
        for slot in order.iter_mut() {
            if *slot != usize::MAX {
                continue;
            }
            match movable.next() {
                Some(row) => *slot = row.grid_index,
                // Unreachable while locked indexes are in range; falling back to
                // the identity keeps a malformed plan from dropping a column.
                None => break,
            }
        }
        for (position, slot) in order.iter_mut().enumerate() {
            if *slot == usize::MAX {
                *slot = position;
            }
        }
        order
    }

    /// Positions, *after* the permutation, that must be hidden.
    pub fn hidden_positions(&self) -> HashSet<usize> {
        let hidden = self.hidden_grid_indices();
        self.order()
            .iter()
            .enumerate()
            .filter(|(_, grid_index)| hidden.contains(grid_index))
            .map(|(position, _)| position)
            .collect()
    }
}

/// Rearrange `values` so that position `i` holds what `order[i]` pointed at.
///
/// `order` must be a permutation of `0..values.len()`; anything else leaves
/// `values` alone, because a partial rearrangement of a grid's columns is worse
/// than none.
pub fn permute<T>(values: &mut Vec<T>, order: &[usize]) -> bool {
    if !is_permutation(order, values.len()) {
        return false;
    }
    let mut taken: Vec<Option<T>> = values.drain(..).map(Some).collect();
    values.reserve(order.len());
    for source in order {
        match taken.get_mut(*source).and_then(Option::take) {
            Some(value) => values.push(value),
            None => return false,
        }
    }
    true
}

/// Give a column that is coming back into view a width again.
///
/// Hiding a column is "set its width to zero", so the widths a grid reads back
/// off itself report zero for everything currently hidden. Writing those back
/// verbatim leaves a column the user just re-checked invisible: listed by the
/// dialog, present in every export, and zero pixels wide on screen. Any column
/// without a width takes its measured one instead; the caller zeroes the
/// still-hidden ones again afterwards, so this does not need to know which are
/// which.
pub fn restore_missing_widths(widths: &mut [i32], measured: &[i32], fallback: i32) {
    for (index, width) in widths.iter_mut().enumerate() {
        if *width <= 0 {
            *width = measured
                .get(index)
                .copied()
                .filter(|w| *w > 0)
                .unwrap_or(fallback);
        }
    }
}

pub fn is_permutation(order: &[usize], len: usize) -> bool {
    if order.len() != len {
        return false;
    }
    let mut seen = vec![false; len];
    for source in order {
        match seen.get_mut(*source) {
            Some(slot) if !*slot => *slot = true,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_coming_back_into_view_gets_its_width_back() {
        // Hiding zeroes a width, so the grid reads zero back for column 1 on
        // the next arrangement. Restoring that verbatim is what left a
        // re-checked column invisible.
        let mut widths = vec![80, 0, 120];
        restore_missing_widths(&mut widths, &[80, 95, 120], 40);
        assert_eq!(widths, vec![80, 95, 120]);
    }

    #[test]
    fn a_width_the_user_dragged_survives_a_rearrangement() {
        // Only missing widths are replaced: a column that is already showing
        // keeps the width the user dragged it to.
        let mut widths = vec![300, 40];
        restore_missing_widths(&mut widths, &[80, 95], 40);
        assert_eq!(widths, vec![300, 40]);
    }

    #[test]
    fn a_revealed_column_without_a_measurement_still_gets_a_usable_width() {
        // A measurement list that is short or itself zero must not hand back
        // the zero we are here to remove.
        let mut widths = vec![0, 0];
        restore_missing_widths(&mut widths, &[0], 40);
        assert_eq!(widths, vec![40, 40]);
    }

    fn row(grid_index: usize, name: &str) -> ColumnLayoutRow {
        ColumnLayoutRow {
            grid_index,
            source_index: grid_index,
            name: name.to_string(),
            visible: true,
            locked: false,
        }
    }

    fn plan(names: &[&str]) -> ColumnLayoutPlan {
        ColumnLayoutPlan::from_rows(
            names
                .iter()
                .enumerate()
                .map(|(index, name)| row(index, name))
                .collect(),
        )
    }

    fn names(plan: &ColumnLayoutPlan) -> Vec<String> {
        plan.rows().iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn hidden_columns_merge_the_grids_own_column_with_the_users() {
        let hidden = HiddenColumns::new(Some(0), HashSet::from([2, 4]));
        assert!(hidden.contains(0));
        assert!(hidden.contains(2));
        assert!(!hidden.contains(1));
        assert!(!hidden.is_empty());
        assert_eq!(hidden.all(), vec![0, 2, 4]);
        assert_eq!(hidden.automatic_column(), Some(0));

        // The technical column counted twice is still one column.
        let overlapping = HiddenColumns::new(Some(2), HashSet::from([2]));
        assert_eq!(overlapping.all(), vec![2]);

        assert!(HiddenColumns::default().is_empty());
        assert!(HiddenColumns::automatic(None).is_empty());
        assert!(!HiddenColumns::automatic(Some(0)).is_empty());
    }

    #[test]
    fn an_untouched_plan_is_the_identity() {
        let plan = plan(&["A", "B", "C"]);
        assert_eq!(plan.order(), vec![0, 1, 2]);
        assert!(plan.hidden_positions().is_empty());
    }

    #[test]
    fn moving_a_column_reorders_the_permutation_too() {
        let mut plan = plan(&["A", "B", "C"]);
        assert_eq!(plan.move_row(2, false), Some(1));
        assert_eq!(names(&plan), vec!["A", "C", "B"]);
        assert_eq!(plan.order(), vec![0, 2, 1]);
    }

    #[test]
    fn a_move_past_either_end_does_nothing() {
        let mut plan = plan(&["A", "B"]);
        assert_eq!(plan.move_row(0, false), None);
        assert_eq!(plan.move_row(1, true), None);
        assert_eq!(plan.move_row(9, true), None);
        assert_eq!(names(&plan), vec!["A", "B"]);
    }

    #[test]
    fn hiding_a_column_marks_its_position_after_the_move() {
        let mut plan = plan(&["A", "B", "C"]);
        plan.set_visible(0, false).expect("hide A");
        assert_eq!(plan.move_row(0, true), Some(1));
        // A is now displayed second, so position 1 is the hidden one.
        assert_eq!(plan.order(), vec![1, 0, 2]);
        assert_eq!(plan.hidden_positions(), HashSet::from([1]));
    }

    #[test]
    fn the_last_visible_column_cannot_be_hidden() {
        let mut plan = plan(&["A", "B"]);
        plan.set_visible(0, false).expect("hide A");
        let refused = plan.set_visible(1, false).expect_err("must refuse");
        assert!(refused.contains("At least one"), "{refused}");
        assert_eq!(plan.visible_count(), 1);
    }

    #[test]
    fn hiding_something_already_hidden_is_not_an_error() {
        let mut plan = plan(&["A", "B"]);
        plan.set_visible(0, false).expect("hide");
        plan.set_visible(0, false).expect("hide again");
        assert_eq!(plan.visible_count(), 1);
    }

    #[test]
    fn reset_undoes_moves_and_hides_together() {
        let mut plan = plan(&["A", "B", "C"]);
        plan.move_row(2, false);
        plan.move_row(1, false);
        plan.set_visible(0, false).expect("hide");
        plan.reset();
        assert_eq!(names(&plan), vec!["A", "B", "C"]);
        assert_eq!(plan.visible_count(), 3);
    }

    #[test]
    fn reset_returns_to_the_arrival_order_not_the_order_it_opened_with() {
        // The dialog can be reopened on a grid that was already rearranged, so
        // source_index — not the current grid index — is what Reset follows.
        let mut plan = ColumnLayoutPlan::from_rows(vec![
            ColumnLayoutRow {
                grid_index: 0,
                source_index: 2,
                name: "C".to_string(),
                visible: true,
                locked: false,
            },
            ColumnLayoutRow {
                grid_index: 1,
                source_index: 0,
                name: "A".to_string(),
                visible: true,
                locked: false,
            },
            ColumnLayoutRow {
                grid_index: 2,
                source_index: 1,
                name: "B".to_string(),
                visible: true,
                locked: false,
            },
        ]);
        plan.reset();
        assert_eq!(names(&plan), vec!["A", "B", "C"]);
        assert_eq!(plan.order(), vec![1, 2, 0]);
    }

    #[test]
    fn a_locked_column_keeps_its_index_while_the_rest_move() {
        let mut rows = vec![
            ColumnLayoutRow {
                grid_index: 0,
                source_index: 0,
                name: "ROWID".to_string(),
                visible: false,
                locked: true,
            },
            row(1, "A"),
            row(2, "B"),
        ];
        rows[1].source_index = 1;
        rows[2].source_index = 2;
        let mut plan = ColumnLayoutPlan::from_rows(rows);
        // The dialog never lists it.
        assert_eq!(names(&plan), vec!["A", "B"]);
        plan.move_row(1, false);
        assert_eq!(names(&plan), vec!["B", "A"]);
        assert_eq!(plan.order(), vec![0, 2, 1]);
        // Reset must not disturb it either.
        plan.reset();
        assert_eq!(plan.order(), vec![0, 1, 2]);
    }

    #[test]
    fn permute_rearranges_and_rejects_a_non_permutation() {
        let mut values = vec!["a", "b", "c"];
        assert!(permute(&mut values, &[2, 0, 1]));
        assert_eq!(values, vec!["c", "a", "b"]);

        let mut values = vec!["a", "b", "c"];
        assert!(!permute(&mut values, &[0, 0, 1]));
        assert_eq!(values, vec!["a", "b", "c"], "left untouched");
        assert!(!permute(&mut values, &[0, 1]));
        assert_eq!(values, vec!["a", "b", "c"], "left untouched");
        assert!(!permute(&mut values, &[0, 1, 5]));
        assert_eq!(values, vec!["a", "b", "c"], "left untouched");
    }

    #[test]
    fn an_empty_grid_permutes_to_nothing_without_complaining() {
        let mut values: Vec<&str> = Vec::new();
        assert!(permute(&mut values, &[]));
        assert!(values.is_empty());
        assert!(is_permutation(&[], 0));
    }
}
